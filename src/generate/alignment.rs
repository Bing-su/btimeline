use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use contracts::{debug_ensures, ensures};
use itertools::Itertools;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffTag, capture_diff_slices};

use super::{GroupKey, Pull, inspect};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SignalKey {
    pub actor_game_id: i64,
    pub role: String,
    pub ability_id: i64,
    pub kind: String,
    pub count: usize,
    pub instances: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Signal {
    pub key: SignalKey,
    pub time_ms: i64,
    pub event_indices: Vec<usize>,
    pub instance_ids: Vec<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Slot {
    pub left: Option<Signal>,
    pub right: Option<Signal>,
    pub evidence: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Segment {
    before_anchor: Option<usize>,
    after_anchor: Option<usize>,
    pub slots: Vec<Slot>,
    relation: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Comparison {
    left: String,
    right: String,
    pub segments: Vec<Segment>,
    pub order_sensitive: bool,
    matched_anchor_count: usize,
    repeated_anchor_keys: Vec<SignalKey>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentReport {
    group: GroupKey,
    inputs: Vec<AlignmentInput>,
    implementation: &'static str,
    comparisons: Vec<Comparison>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AlignmentInput {
    file: String,
    game_version: i64,
    log_version: i64,
}

// Every occurrence contributes to exactly one grouped signal, even when helpers interleave.
#[debug_ensures(ret.iter().map(|signal| signal.key.count).sum::<usize>() == pull.occurrences.len())]
pub(super) fn signals(pull: &Pull) -> Vec<Signal> {
    let mut result: Vec<Signal> = Vec::new();
    let mut positions: BTreeMap<(usize, i64, i64, &str), usize> = BTreeMap::new();
    let mut instances: Vec<BTreeSet<i64>> = Vec::new();
    for row in &pull.occurrences {
        let identity = (
            row.simultaneous,
            row.actor_game_id,
            row.ability_id,
            row.kind.as_str(),
        );
        if let Some(&position) = positions.get(&identity) {
            if let Some(signal) = result.get_mut(position) {
                signal.key.count += 1;
                signal.event_indices.push(row.event_index);
                if let Some(instance) = row.instance
                    && let Some(seen) = instances.get_mut(position)
                {
                    seen.insert(instance);
                    signal.key.instances = seen.len();
                    signal.instance_ids = seen.iter().copied().collect();
                }
            }
            continue;
        }
        positions.insert(identity, result.len());
        let seen: BTreeSet<i64> = row.instance.into_iter().collect();
        result.push(Signal {
            key: SignalKey {
                actor_game_id: row.actor_game_id,
                role: row.role.clone(),
                ability_id: row.ability_id,
                kind: row.kind.clone(),
                count: 1,
                instances: seen.len(),
            },
            time_ms: row.relative_ms,
            event_indices: vec![row.event_index],
            instance_ids: seen.iter().copied().collect(),
        });
        instances.push(seen);
    }
    result
}

fn boss_anchors(signals: &[Signal]) -> Vec<usize> {
    signals
        .iter()
        .enumerate()
        .filter(|(_, signal)| is_boss_cast(signal))
        .map(|(index, _)| index)
        .collect()
}

fn is_boss_cast(signal: &Signal) -> bool {
    signal.key.role.ends_with("/Boss") && signal.key.kind == "cast"
}

// Myers/LCS keeps repeated occurrences as separate ordered positions.
// A successful alignment consumes each input position exactly once.
#[debug_ensures(ret.as_ref().map_or(true, |pairs| {
    pairs.iter().filter(|(left, _)| left.is_some()).count() == left.len()
        && pairs.iter().filter(|(_, right)| right.is_some()).count() == right.len()
}))]
fn paired_keys(
    left: &[SignalKey],
    right: &[SignalKey],
) -> Result<Vec<(Option<usize>, Option<usize>)>> {
    let mut pairs = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, left, right) {
        let old = op.old_range();
        let new = op.new_range();
        match op.tag() {
            DiffTag::Equal => {
                ensure!(old.len() == new.len(), "Invalid equal alignment span");
                pairs.extend(old.zip(new).map(|(i, j)| (Some(i), Some(j))));
            }
            DiffTag::Delete => pairs.extend(old.map(|i| (Some(i), None))),
            DiffTag::Insert => pairs.extend(new.map(|j| (None, Some(j)))),
            DiffTag::Replace => {
                pairs.extend(old.map(|i| (Some(i), None)));
                pairs.extend(new.map(|j| (None, Some(j))));
            }
        }
    }
    Ok(pairs)
}

fn pairs(left: &[Signal], right: &[Signal]) -> Result<Vec<(Option<usize>, Option<usize>)>> {
    paired_keys(
        &left.iter().map(|s| s.key.clone()).collect_vec(),
        &right.iter().map(|s| s.key.clone()).collect_vec(),
    )
}

fn boss_pairs(left: &[Signal], right: &[Signal]) -> Result<Vec<(Option<usize>, Option<usize>)>> {
    let left_anchors = boss_anchors(left);
    let right_anchors = boss_anchors(right);
    paired_keys(
        &left_anchors
            .iter()
            .filter_map(|&i| left.get(i))
            .map(|s| s.key.clone())
            .collect_vec(),
        &right_anchors
            .iter()
            .filter_map(|&i| right.get(i))
            .map(|s| s.key.clone())
            .collect_vec(),
    )
    .map(|aligned| {
        aligned
            .into_iter()
            .map(|(i, j)| {
                (
                    i.and_then(|i| left_anchors.get(i)).copied(),
                    j.and_then(|j| right_anchors.get(j)).copied(),
                )
            })
            .collect()
    })
}

fn observed_len(
    signals: &[Signal],
    peer_signals: &[Signal],
    peer: &Pull,
    entry: (i64, i64),
) -> usize {
    if peer.kill {
        return signals.len();
    }
    // An identical observed prefix can drift, e.g. A→B at 1s→25s versus 1s→18s before a 20s wipe.
    // Beyond that prefix, keep the wipe bound so an early branch cannot match a later repeat.
    let (own_entry, peer_entry) = signals
        .iter()
        .zip(peer_signals)
        .take_while(|(a, b)| a.key == b.key)
        .last()
        .map_or(entry, |(a, b)| (a.time_ms, b.time_ms));
    signals
        .iter()
        .take_while(|signal| signal.time_ms - own_entry <= peer.end_ms - peer_entry)
        .count()
}

fn observed_pairs(
    left: &[Signal],
    right: &[Signal],
    left_pull: &Pull,
    right_pull: &Pull,
    mut entry: (i64, i64),
    pair_signals: impl Fn(&[Signal], &[Signal]) -> Result<Vec<(Option<usize>, Option<usize>)>>,
) -> Result<Vec<(Option<usize>, Option<usize>)>> {
    let mut result = Vec::new();
    let (mut left_start, mut right_start) = (0, 0);
    loop {
        let left_remaining = left.get(left_start..).context("Invalid left suffix")?;
        let right_remaining = right.get(right_start..).context("Invalid right suffix")?;
        let left_len = observed_len(left_remaining, right_remaining, right_pull, entry);
        let right_len = observed_len(
            right_remaining,
            left_remaining,
            left_pull,
            (entry.1, entry.0),
        );
        let aligned = pair_signals(
            left_remaining
                .get(..left_len)
                .context("Invalid observed left prefix")?,
            right_remaining
                .get(..right_len)
                .context("Invalid observed right prefix")?,
        )?;
        let last_match = aligned
            .iter()
            .enumerate()
            .rev()
            .find_map(|(position, &(i, j))| Some((position, i?, j?)));
        if let Some((position, i, j)) = last_match {
            result.extend(
                aligned
                    .iter()
                    .take(position + 1)
                    .map(|&(i, j)| (i.map(|i| left_start + i), j.map(|j| right_start + j))),
            );
            // Resume at the observed common successor; X/Y→C can establish a new drifting B prefix.
            entry = (
                left_remaining.get(i).context("Missing left match")?.time_ms,
                right_remaining
                    .get(j)
                    .context("Missing right match")?
                    .time_ms,
            );
            left_start += i + 1;
            right_start += j + 1;
        } else {
            result.extend(
                aligned
                    .into_iter()
                    .map(|(i, j)| (i.map(|i| left_start + i), j.map(|j| right_start + j))),
            );
            // Retain excluded suffixes as source evidence, rather than deleting their occurrences.
            result.extend((left_len..left_remaining.len()).map(|i| (Some(left_start + i), None)));
            result
                .extend((right_len..right_remaining.len()).map(|i| (None, Some(right_start + i))));
            return Ok(result);
        }
    }
}

fn slot(
    left: Option<&Signal>,
    right: Option<&Signal>,
    left_pull: &Pull,
    right_pull: &Pull,
    has_common_successor: bool,
    before_anchor_times: Option<(i64, i64)>,
) -> Slot {
    let evidence = match (left, right) {
        (Some(a), Some(b)) if a.key == b.key => "matched",
        (Some(_), Some(_)) => "unresolved",
        (Some(a), None)
            if !has_common_successor
                && before_anchor_times.is_some_and(|(left_ms, right_ms)| {
                    suffix_unobserved(
                        right_pull.kill,
                        a.time_ms - left_ms,
                        right_pull.end_ms - right_ms,
                    )
                }) =>
        {
            "rightUnobservedAfterWipe"
        }
        (None, Some(b))
            if !has_common_successor
                && before_anchor_times.is_some_and(|(left_ms, right_ms)| {
                    suffix_unobserved(
                        left_pull.kill,
                        b.time_ms - right_ms,
                        left_pull.end_ms - left_ms,
                    )
                }) =>
        {
            "leftUnobservedAfterWipe"
        }
        _ => "observedOnlyOnOnePath",
    };
    Slot {
        left: left.cloned(),
        right: right.cloned(),
        evidence,
    }
}

fn section(
    left: &[Signal],
    right: &[Signal],
    left_pull: &Pull,
    right_pull: &Pull,
    before_anchor: Option<usize>,
    after_anchor: Option<usize>,
    before_anchor_times: Option<(i64, i64)>,
) -> Result<Segment> {
    // Before any shared cast, elapsed time starts at the collected fight boundary.
    // Each matched signal refines that boundary, e.g. 10s→15s completes after a 19s→20s wipe.
    let mut censor_reference = Some(before_anchor_times.unwrap_or((0, 0)));
    let aligned = if after_anchor.is_none() {
        observed_pairs(
            left,
            right,
            left_pull,
            right_pull,
            censor_reference.context("Missing entry")?,
            pairs,
        )?
    } else {
        pairs(left, right)?
    };
    let slots = aligned
        .into_iter()
        .map(|(i, j)| {
            let left_signal = i.and_then(|index| left.get(index));
            let right_signal = j.and_then(|index| right.get(index));
            let result = slot(
                left_signal,
                right_signal,
                left_pull,
                right_pull,
                after_anchor.is_some(),
                censor_reference,
            );
            if let (Some(a), Some(b)) = (left_signal, right_signal)
                && a.key == b.key
            {
                censor_reference = Some((a.time_ms, b.time_ms));
            }
            result
        })
        .collect();
    Ok(Segment {
        before_anchor,
        after_anchor,
        slots,
        relation: "common",
    })
}

fn align_segments(left_pull: &Pull, right_pull: &Pull) -> Result<Vec<Segment>> {
    let mut left = signals(left_pull);
    let mut right = signals(right_pull);
    // Keep simultaneous helpers on the same side of each anchor: A+H equals H+A.
    // Preserve helper-only path context and raw replay order.
    // ponytail: normalize single-anchor batches; multiple anchors need bundle-aware segmentation.
    for signals in [&mut left, &mut right] {
        for batch in signals.chunk_by_mut(|a, b| a.time_ms == b.time_ms) {
            if batch.iter().filter(|s| is_boss_cast(s)).count() == 1 {
                batch.sort_by(|a, b| a.key.cmp(&b.key));
            }
        }
    }
    // An identical opening boss signal corrects pull-start offset before finding repeat anchors.
    let entry = match (left.first(), right.first()) {
        (Some(a), Some(b)) if a.key == b.key && a.key.role.ends_with("/Boss") => {
            (a.time_ms, b.time_ms)
        }
        _ => (0, 0),
    };
    // Keep helper context when bounding boss candidates; A→helpers→B differs from an early A→B wipe.
    let anchor_pairs = observed_pairs(&left, &right, left_pull, right_pull, entry, boss_pairs)?;
    let matched: Vec<_> = anchor_pairs
        .into_iter()
        .filter_map(|(left, right)| Some((left?, right?)))
        .collect();
    let mut segments = Vec::new();
    let (mut left_start, mut right_start, mut before_anchor) = (0, 0, None);
    let mut before_anchor_times = Some(entry);
    for (anchor_number, &(li, rj)) in matched.iter().enumerate() {
        let mut segment = section(
            left.get(left_start..li)
                .context("Invalid left anchor order")?,
            right
                .get(right_start..rj)
                .context("Invalid right anchor order")?,
            left_pull,
            right_pull,
            before_anchor,
            Some(anchor_number),
            before_anchor_times,
        )?;
        segment.slots.push(slot(
            left.get(li),
            right.get(rj),
            left_pull,
            right_pull,
            true,
            before_anchor_times,
        ));
        segments.push(segment);
        left_start = li + 1;
        right_start = rj + 1;
        before_anchor = Some(anchor_number);
        before_anchor_times = Some((
            left.get(li).context("Missing left anchor")?.time_ms,
            right.get(rj).context("Missing right anchor")?.time_ms,
        ));
    }
    segments.push(section(
        left.get(left_start..).context("Invalid left suffix")?,
        right.get(right_start..).context("Invalid right suffix")?,
        left_pull,
        right_pull,
        before_anchor,
        None,
        before_anchor_times,
    )?);
    Ok(segments)
}

fn segment_relation(slots: &[Slot]) -> &'static str {
    if slots
        .iter()
        .any(|slot| matches!(slot.evidence, "observedOnlyOnOnePath" | "unresolved"))
    {
        "divergentObservedPaths"
    } else if slots
        .iter()
        .any(|slot| slot.evidence.contains("UnobservedAfterWipe"))
    {
        "censoredAfterWipe"
    } else if slots.iter().any(|slot| slot.evidence == "repeatCandidate") {
        "repeatCandidate"
    } else {
        "common"
    }
}

pub(super) fn compare(left_pull: &Pull, right_pull: &Pull) -> Result<Comparison> {
    let segments = align_segments(left_pull, right_pull)?;
    let reversed = align_segments(right_pull, left_pull)?;
    // Only correspondence changes count: independent exclusive paths may interleave differently.
    let forward: Vec<_> = segments
        .iter()
        .flat_map(|s| &s.slots)
        .filter_map(|s| {
            Some((
                s.left.as_ref()?.event_indices.first().copied()?,
                s.right.as_ref()?.event_indices.first().copied()?,
            ))
        })
        .collect();
    let reverse: Vec<_> = reversed
        .iter()
        .flat_map(|s| &s.slots)
        .filter_map(|s| {
            Some((
                s.right.as_ref()?.event_indices.first().copied()?,
                s.left.as_ref()?.event_indices.first().copied()?,
            ))
        })
        .collect();
    let repeated_anchor_keys: Vec<SignalKey> = signals(left_pull)
        .into_iter()
        .chain(signals(right_pull))
        .filter(is_boss_cast)
        .counts_by(|signal| signal.key)
        .into_iter()
        .filter_map(|(key, count)| (count > 2).then_some(key))
        // counts_by uses a hash map; sort keys to keep reports independent of hash iteration order.
        .sorted()
        .collect();
    let matched_anchor_count = segments.len().saturating_sub(1);
    let mut segments = segments;
    for segment in &mut segments {
        for slot in &mut segment.slots {
            if slot.evidence == "matched"
                && slot
                    .left
                    .as_ref()
                    .is_some_and(|signal| repeated_anchor_keys.contains(&signal.key))
            {
                slot.evidence = "repeatCandidate";
            }
        }
        segment.relation = segment_relation(&segment.slots);
    }
    Ok(Comparison {
        left: left_pull.file.clone(),
        right: right_pull.file.clone(),
        segments,
        order_sensitive: forward != reverse,
        matched_anchor_count,
        repeated_anchor_keys,
    })
}

pub fn align(paths: &[impl AsRef<Path>]) -> Result<AlignmentReport> {
    ensure!(paths.len() >= 2, "Supply at least two input files");
    let mut groups = inspect(paths)?;
    ensure!(
        groups.len() == 1,
        "Alignment requires one compatible input group"
    );
    let group = groups.pop().context("Missing group")?;
    let mut pulls = group.pulls;
    pulls.sort_by(|a, b| (&a.report, a.fight, &a.file).cmp(&(&b.report, b.fight, &b.file)));
    // ponytail: all pairs preserve path evidence; use an indexed graph if groups become large.
    let pairs: Vec<_> = pulls.iter().array_combinations().collect();
    let comparisons = pairs
        .par_iter()
        .map(|[left, right]| compare(left, right))
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    Ok(AlignmentReport {
        group: group.key,
        inputs: pulls
            .iter()
            .map(|pull| AlignmentInput {
                file: pull.file.clone(),
                game_version: pull.game_version,
                log_version: pull.log_version,
            })
            .collect(),
        implementation: "similar 3 Myers/LCS",
        comparisons,
    })
}

// Compare elapsed time since the shared anchor; raw pull times can drift apart.
#[ensures(ret == (!kill && candidate_elapsed_ms > observed_elapsed_ms))]
fn suffix_unobserved(kill: bool, candidate_elapsed_ms: i64, observed_elapsed_ms: i64) -> bool {
    !kill && candidate_elapsed_ms > observed_elapsed_ms
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate::Occurrence;

    fn pull(file: &str, kill: bool, end_ms: i64, rows: &[(i64, i64, &str, i64, &str)]) -> Pull {
        Pull {
            file: file.into(),
            report: file.into(),
            revision: 1,
            game_version: 1,
            log_version: 76,
            fight: 1,
            name: "Synthetic".into(),
            kill,
            end_ms,
            occurrences: rows
                .iter()
                .enumerate()
                .map(|(index, &(time, actor, role, ability, kind))| Occurrence {
                    event_index: index,
                    timestamp_ms: time,
                    relative_ms: time,
                    simultaneous: index + 1,
                    kind: kind.into(),
                    actor_id: actor,
                    actor_game_id: actor,
                    role: role.into(),
                    instance: Some(actor),
                    ability_id: ability,
                    start_event_index: None,
                    start_timestamp_ms: None,
                    completion_event_index: None,
                })
                .collect(),
            actors: Default::default(),
        }
    }

    #[test]
    fn alternatives_keep_their_order_and_common_successor() {
        let boss = "NPC/Boss";
        let helper = "NPC/NPC";
        let left = pull(
            "a",
            true,
            500,
            &[
                (100, 1, boss, 1, "cast"),
                (200, 2, helper, 10, "begincast"),
                (250, 2, helper, 10, "cast"),
                (400, 1, boss, 2, "cast"),
            ],
        );
        let right = pull(
            "b",
            true,
            500,
            &[
                (100, 1, boss, 1, "cast"),
                (200, 2, helper, 11, "begincast"),
                (250, 2, helper, 11, "cast"),
                (400, 1, boss, 2, "cast"),
            ],
        );
        let comparison = compare(&left, &right).unwrap();
        assert!(!comparison.order_sensitive);
        assert_eq!(comparison.segments.len(), 3);
        assert_eq!(comparison.segments[1].relation, "divergentObservedPaths");
        let middle = &comparison.segments[1].slots;
        assert_eq!(
            middle.iter().map(|slot| slot.evidence).collect::<Vec<_>>(),
            [
                "observedOnlyOnOnePath",
                "observedOnlyOnOnePath",
                "observedOnlyOnOnePath",
                "observedOnlyOnOnePath",
                "matched",
            ]
        );
        assert_eq!(middle[0].left.as_ref().unwrap().key.kind, "begincast");
        assert_eq!(middle[2].right.as_ref().unwrap().key.kind, "begincast");
    }

    #[test]
    fn repeated_anchor_keys_are_sorted_after_counting() {
        let rows = [30, 10, 20, 30, 20, 10, 40]
            .into_iter()
            .enumerate()
            .map(|(i, id)| ((i as i64 + 1) * 100, 1, "NPC/Boss", id, "cast"))
            .collect_vec();
        let left = pull("a", true, 800, &rows);
        let right = pull("b", true, 800, &rows);
        let comparison = compare(&left, &right).unwrap();
        // Encounter order starts with 30; the report sorts repeated keys and excludes the single 40.
        assert_eq!(
            comparison
                .repeated_anchor_keys
                .iter()
                .map(|key| key.ability_id)
                .collect_vec(),
            [10, 20, 30]
        );
    }

    #[test]
    fn repeated_anchor_and_wipe_suffix_stay_distinct() {
        let boss = "NPC/Boss";
        let left = pull(
            "a",
            true,
            500,
            &[
                (100, 1, boss, 1, "cast"),
                (200, 1, boss, 1, "cast"),
                (400, 1, boss, 1, "cast"),
            ],
        );
        let right = pull(
            "b",
            false,
            250,
            &[(100, 1, boss, 1, "cast"), (200, 1, boss, 1, "cast")],
        );
        let comparison = compare(&left, &right).unwrap();
        let slots: Vec<_> = comparison.segments.iter().flat_map(|s| &s.slots).collect();
        assert_eq!(
            slots
                .iter()
                .filter(|s| s.evidence == "repeatCandidate")
                .count(),
            2
        );
        assert_eq!(
            slots
                .iter()
                .filter(|s| s.evidence == "rightUnobservedAfterWipe")
                .count(),
            1
        );
        assert_eq!(slots.iter().filter(|s| s.left.is_some()).count(), 3);
    }

    #[test]
    fn delayed_anchor_does_not_turn_observed_path_difference_into_wipe_suffix() {
        let left = pull(
            "a",
            true,
            7000,
            &[
                (4000, 1, "NPC/Boss", 1, "cast"),
                (6000, 2, "NPC/NPC", 10, "cast"),
            ],
        );
        let right = pull("b", false, 5000, &[(1000, 1, "NPC/Boss", 1, "cast")]);
        let comparison = compare(&left, &right).unwrap();
        assert_eq!(comparison.segments[1].relation, "divergentObservedPaths");
        assert_eq!(
            comparison.segments[1].slots[0].evidence,
            "observedOnlyOnOnePath"
        );
    }

    #[test]
    fn observed_successor_prevents_false_wipe_censoring() {
        let left = pull(
            "a",
            true,
            500,
            &[
                (100, 1, "NPC/Boss", 1, "cast"),
                (300, 2, "NPC/NPC", 10, "cast"),
                (400, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let right = pull(
            "b",
            false,
            250,
            &[
                (100, 1, "NPC/Boss", 1, "cast"),
                (200, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let comparison = compare(&left, &right).unwrap();
        assert_eq!(comparison.segments[1].relation, "divergentObservedPaths");
        assert_eq!(
            comparison.segments[1].slots[0].evidence,
            "observedOnlyOnOnePath"
        );
    }

    #[test]
    fn simultaneous_anchor_order_preserves_repeated_helper_correspondence() {
        // Reordered A+H batches must keep each Grand Cross with its own round, in both directions.
        let boss = "NPC/Boss";
        let helper = "NPC/NPC";
        let left = pull(
            "a",
            true,
            4000,
            &[
                (500, 2, helper, 20, "begincast"),
                (1000, 1, boss, 1, "cast"),
                (1000, 2, helper, 10, "cast"),
                (1400, 2, helper, 20, "cast"),
                (1500, 2, helper, 20, "begincast"),
                (2000, 1, boss, 1, "cast"),
                (2000, 2, helper, 10, "cast"),
                (2400, 2, helper, 20, "cast"),
                (3500, 1, boss, 2, "cast"),
            ],
        );
        let right = pull(
            "b",
            false,
            3000,
            &[
                (600, 2, helper, 20, "begincast"),
                (1100, 2, helper, 10, "cast"),
                (1100, 1, boss, 1, "cast"),
                (1500, 2, helper, 20, "cast"),
                (1600, 2, helper, 20, "begincast"),
                (2100, 2, helper, 10, "cast"),
                (2100, 1, boss, 1, "cast"),
                (2500, 2, helper, 20, "cast"),
            ],
        );
        for (a, b) in [(&left, &right), (&right, &left)] {
            let comparison = compare(a, b).unwrap();
            assert!(!comparison.order_sensitive);
            let slots: Vec<_> = comparison.segments.iter().flat_map(|s| &s.slots).collect();
            assert_eq!(slots.len(), left.occurrences.len());
            for slot in slots {
                match (&slot.left, &slot.right) {
                    (Some(x), Some(y)) => {
                        assert_eq!(x.key, y.key);
                        assert_eq!((x.time_ms - y.time_ms).abs(), 100);
                        // Retain provenance from each original input order.
                        assert_eq!(a.occurrences[x.event_indices[0]].relative_ms, x.time_ms);
                        assert_eq!(b.occurrences[y.event_indices[0]].relative_ms, y.time_ms);
                    }
                    _ => assert!(slot.evidence.contains("UnobservedAfterWipe")),
                }
            }
        }
    }

    #[test]
    fn simultaneous_interleaved_instances_keep_count() {
        let mut pull = pull(
            "a",
            true,
            500,
            &[
                (100, 2, "NPC/NPC", 10, "cast"),
                (100, 3, "NPC/NPC", 11, "cast"),
                (100, 2, "NPC/NPC", 10, "cast"),
            ],
        );
        for row in &mut pull.occurrences {
            row.simultaneous = 1;
        }
        pull.occurrences[2].instance = Some(20);
        let signals = signals(&pull);
        assert_eq!(signals.len(), 2);
        assert_eq!(signals[0].key.count, 2);
        assert_eq!(signals[0].key.instances, 2);
        assert_eq!(signals[0].event_indices, [0, 2]);
        assert_eq!(signals[0].instance_ids, [2, 20]);
    }

    #[test]
    fn wipe_during_first_cast_uses_the_observed_start_as_its_time_reference() {
        let left = pull(
            "a",
            true,
            40000,
            &[
                (10000, 1, "NPC/Boss", 1, "begincast"),
                (15000, 1, "NPC/Boss", 1, "cast"),
                (30000, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let right = pull("b", false, 20000, &[(19000, 1, "NPC/Boss", 1, "begincast")]);
        let comparison = compare(&left, &right).unwrap();
        let slots: Vec<_> = comparison.segments.iter().flat_map(|s| &s.slots).collect();
        assert_eq!(slots[0].evidence, "matched");
        assert_eq!(slots[1].evidence, "rightUnobservedAfterWipe");
        assert_eq!(slots[2].evidence, "rightUnobservedAfterWipe");
    }

    #[test]
    fn observation_limit_preserves_an_opening_start_when_pull_offsets_differ() {
        // The 20s opening is the same observed start as 10s, despite the other pull ending at 11s.
        let left = pull("a", false, 11000, &[(10000, 1, "NPC/Boss", 1, "begincast")]);
        let right = pull(
            "b",
            true,
            30000,
            &[
                (20000, 1, "NPC/Boss", 1, "begincast"),
                (25000, 1, "NPC/Boss", 1, "cast"),
            ],
        );
        for (a, b) in [(&left, &right), (&right, &left)] {
            let comparison = compare(a, b).unwrap();
            let slots: Vec<_> = comparison.segments.iter().flat_map(|s| &s.slots).collect();
            assert_eq!(slots[0].evidence, "matched");
            assert!(slots[1].evidence.contains("UnobservedAfterWipe"));
        }
    }

    #[test]
    fn wipe_before_any_signal_censors_only_events_after_observation_ends() {
        let left = pull(
            "a",
            true,
            20000,
            &[
                (3000, 1, "NPC/Boss", 1, "cast"),
                (10000, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let right = pull("b", false, 5000, &[]);
        let comparison = compare(&left, &right).unwrap();
        let slots: Vec<_> = comparison.segments.iter().flat_map(|s| &s.slots).collect();
        assert_eq!(slots[0].evidence, "observedOnlyOnOnePath");
        assert_eq!(slots[1].evidence, "rightUnobservedAfterWipe");
    }

    #[test]
    fn helper_divergence_prevents_matching_an_early_cast_to_a_later_boss_cast() {
        // Boss-only A→B order must not erase the helper path between A and the later B.
        let left = pull(
            "a",
            true,
            30000,
            &[
                (1000, 1, "NPC/Boss", 1, "cast"),
                (2000, 2, "NPC/NPC", 10, "cast"),
                (5000, 2, "NPC/NPC", 11, "cast"),
                (20000, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let right = pull(
            "b",
            false,
            3000,
            &[
                (1000, 1, "NPC/Boss", 1, "cast"),
                (2000, 1, "NPC/Boss", 2, "cast"),
            ],
        );
        let comparison = compare(&left, &right).unwrap();
        assert!(comparison.segments.iter().flat_map(|segment| &segment.slots).all(|slot| {
            !matches!((&slot.left, &slot.right), (Some(a), Some(_)) if a.key.ability_id == 2)
        }));
    }
}

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use anyhow::{Context, Result, ensure};
use contracts::{debug_ensures, ensures};
use serde::Serialize;
use similar::{Algorithm, DiffTag, capture_diff_slices};

use super::{GroupKey, Pull, inspect};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignalKey {
    actor_game_id: i64,
    role: String,
    ability_id: i64,
    kind: String,
    count: usize,
    instances: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Signal {
    key: SignalKey,
    time_ms: i64,
    event_indices: Vec<usize>,
    instance_ids: Vec<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    left: Option<Signal>,
    right: Option<Signal>,
    evidence: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Segment {
    before_anchor: Option<usize>,
    after_anchor: Option<usize>,
    slots: Vec<Slot>,
    relation: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Comparison {
    left: String,
    right: String,
    segments: Vec<Segment>,
    order_sensitive: bool,
    matched_anchor_count: usize,
    repeated_anchor_keys: Vec<SignalKey>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentReport {
    group: GroupKey,
    implementation: &'static str,
    comparisons: Vec<Comparison>,
}

// Every occurrence contributes to exactly one grouped signal, even when helpers interleave.
#[debug_ensures(ret.iter().map(|signal| signal.key.count).sum::<usize>() == pull.occurrences.len())]
fn signals(pull: &Pull) -> Vec<Signal> {
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
        &left.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
        &right.iter().map(|s| s.key.clone()).collect::<Vec<_>>(),
    )
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
    let slots = pairs(left, right)?
        .into_iter()
        .map(|(i, j)| {
            slot(
                i.and_then(|index| left.get(index)),
                j.and_then(|index| right.get(index)),
                left_pull,
                right_pull,
                after_anchor.is_some(),
                before_anchor_times,
            )
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
    let left = signals(left_pull);
    let right = signals(right_pull);
    let left_anchors = boss_anchors(&left);
    let right_anchors = boss_anchors(&right);
    let anchor_pairs = paired_keys(
        &left_anchors
            .iter()
            .filter_map(|&i| left.get(i))
            .map(|s| s.key.clone())
            .collect::<Vec<_>>(),
        &right_anchors
            .iter()
            .filter_map(|&i| right.get(i))
            .map(|s| s.key.clone())
            .collect::<Vec<_>>(),
    )?;
    let matched: Vec<_> = anchor_pairs
        .into_iter()
        .filter_map(|(left, right)| Some((*left_anchors.get(left?)?, *right_anchors.get(right?)?)))
        .collect();
    let mut segments = Vec::new();
    let (mut left_start, mut right_start, mut before_anchor) = (0, 0, None);
    let mut before_anchor_times = None;
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
        before_anchor_times = Some((left[li].time_ms, right[rj].time_ms));
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

fn compare(left_pull: &Pull, right_pull: &Pull) -> Result<Comparison> {
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
    let mut counts = BTreeMap::new();
    for signal in signals(left_pull)
        .into_iter()
        .chain(signals(right_pull))
        .filter(is_boss_cast)
    {
        *counts.entry(signal.key).or_insert(0usize) += 1;
    }
    let repeated_anchor_keys: Vec<SignalKey> = counts
        .into_iter()
        .filter_map(|(key, count)| (count > 2).then_some(key))
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
    let mut comparisons = Vec::new();
    // ponytail: all pairs preserve path evidence; use an indexed graph if groups become large.
    for (index, left) in pulls.iter().enumerate() {
        for right in pulls.iter().skip(index + 1) {
            comparisons.push(compare(left, right)?);
        }
    }
    Ok(AlignmentReport {
        group: group.key,
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
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use indicatif::{ParallelProgressIterator, ProgressStyle};
use itertools::Itertools;
use path_slash::PathBufExt as _;
use rayon::prelude::*;
use serde_json::Value;

use super::alignment::{self, Signal, SignalKey};
use super::report::{
    Alignment,
    ConflictingEvent,
    MultiBlock,
    MultiConflict,
    MultiReport,
    MultiSlot,
    ObservedPath,
    OmittedSignal,
    OutputCoverage,
    Sample,
    SensitivePair,
    TimeStatistics,
    Validation,
};
use super::{GenerateMode, Group, Pull, draft, input, load_one};
use crate::fflogs::model::CollectedLog;
use crate::timeline::{Ability, Entry, FieldPattern, LogType, NetworkSync, Sync};

#[derive(Default)]
pub(super) struct Correspondence {
    pub matched: BTreeMap<usize, usize>,
    pub censored: BTreeSet<usize>,
}

fn index(signal: &Signal) -> Result<usize> {
    signal
        .event_indices
        .first()
        .copied()
        .context("Signal has no source event")
}

fn same_context(a: &SignalKey, b: &SignalKey) -> bool {
    a.actor_game_id == b.actor_game_id
        && a.actor_name == b.actor_name
        && a.role == b.role
        && a.kind == b.kind
        && a.count == b.count
        && a.instances == b.instances
}

fn alternatives(left: &[&Signal], right: &[&Signal]) -> bool {
    if left.is_empty() || left.len() != right.len() {
        return false;
    }
    let mut changes = BTreeSet::new();
    for (a, b) in left.iter().zip(right) {
        if !same_context(&a.key, &b.key) {
            return false;
        }
        if a.key.ability_id != b.key.ability_id {
            changes.insert((
                a.key.actor_game_id,
                &a.key.actor_name,
                &a.key.role,
                a.key.ability_id,
                b.key.ability_id,
            ));
        }
    }
    // One ability substitution may include its start and completion (X-start, X-cast → Y-start, Y-cast).
    // Two substitutions, e.g. X,A → Y,B, carry a dependent successor and stay unresolved.
    changes.len() == 1
}

// Helpers stop only after every interacting boss dies, e.g. one defeated overlapping boss is insufficient.
pub(super) fn boss_defeat(pull: &Pull, start: i64, end: i64) -> Option<i64> {
    let bosses: BTreeSet<_> = pull
        .boss_interactions
        .iter()
        .filter(|&&(_, at)| start <= at && at <= end)
        .map(|&(actor, _)| actor)
        .collect();
    bosses
        .iter()
        .map(|actor| {
            pull.boss_deaths
                .iter()
                .filter(|&&(id, at)| id == *actor && start <= at && at <= end)
                .map(|&(_, at)| at)
                .min()
        })
        .collect::<Option<Vec<_>>>()
        .and_then(|times| times.into_iter().max())
}

pub(super) fn correspondence(left: &Pull, right: &Pull) -> Result<(Correspondence, bool)> {
    if left.boss_spans.is_empty() && !left.phase_spans.is_empty() && !right.phase_spans.is_empty() {
        let mut result = Correspondence::default();
        let mut sensitive = false;
        for (position, &(id, lo, hi)) in left.phase_spans.iter().enumerate() {
            let round = left
                .phase_spans
                .iter()
                .take(position)
                .filter(|&&(phase, _, _)| phase == id)
                .count();
            let Some(&(_, other_lo, other_hi)) = right
                .phase_spans
                .iter()
                .filter(|&&(phase, _, _)| phase == id)
                .nth(round)
            else {
                if !right.kill
                    && position >= right.phase_spans.len()
                    && left
                        .phase_spans
                        .iter()
                        .zip(&right.phase_spans)
                        .all(|(a, b)| a.0 == b.0)
                {
                    result.censored.extend(
                        alignment::signals(left)
                            .into_iter()
                            .filter(|signal| lo <= signal.time_ms && signal.time_ms <= hi)
                            .filter_map(|signal| signal.event_indices.first().copied()),
                    );
                }
                continue;
            };
            let mut a = left.clone();
            let mut b = right.clone();
            for (pull, start, end) in [(&mut a, lo, hi), (&mut b, other_lo, other_hi)] {
                pull.occurrences
                    .retain(|row| start <= row.relative_ms && row.relative_ms <= end);
                pull.end_ms = end;
            }
            let (mut relation, order_sensitive) = path_correspondence(&a, &b)?;
            let own = alignment::signals(&a);
            let peer = alignment::signals(&b);
            // Use unique phase-relative timing to break repeated-ID ties, e.g. swapped Past/Future rounds 20s apart.
            for signal in &own {
                let source = index(signal)?;
                let candidates: Vec<_> = peer
                    .iter()
                    .filter(|other| {
                        alignment::match_key(signal) == alignment::match_key(other)
                            && (signal.time_ms - lo).abs_diff(other.time_ms - other_lo) <= 2500
                    })
                    .collect();
                if let [other] = candidates.as_slice()
                    && own
                        .iter()
                        .filter(|candidate| {
                            alignment::match_key(candidate) == alignment::match_key(other)
                                && (candidate.time_ms - lo).abs_diff(other.time_ms - other_lo)
                                    <= 2500
                        })
                        .count()
                        == 1
                {
                    let target = index(other)?;
                    relation.matched.retain(|_, mapped| *mapped != target);
                    relation.matched.insert(source, target);
                    relation.censored.remove(&source);
                } else if relation
                    .matched
                    .get(&source)
                    .and_then(|target| peer.iter().find(|other| index(other).ok() == Some(*target)))
                    .is_some_and(|other| {
                        (signal.time_ms - lo).abs_diff(other.time_ms - other_lo) > 2500
                    })
                {
                    relation.matched.remove(&source);
                }
            }
            if let Some(defeated) = boss_defeat(&b, other_lo, other_hi) {
                for signal in &own {
                    let source = index(signal)?;
                    if relation.matched.contains_key(&source) {
                        continue;
                    }
                    let clock = own
                        .iter()
                        .filter(|anchor| {
                            anchor.key.kind == "cast"
                                && anchor.key.role.ends_with("/Boss")
                                && anchor.time_ms <= signal.time_ms
                        })
                        .filter_map(|anchor| {
                            let target = relation.matched.get(&index(anchor).ok()?)?;
                            let other = peer
                                .iter()
                                .find(|other| index(other).ok() == Some(*target))?;
                            Some((anchor.time_ms, other.time_ms))
                        })
                        .max_by_key(|&(at, _)| at)
                        .unwrap_or((lo, other_lo));
                    // Explicit defeat bounds the suffix, e.g. a shorter P5 clear cannot observe its later enrage.
                    if signal.time_ms - clock.0 + clock.1 > defeated {
                        relation.censored.insert(source);
                    }
                }
            }
            result.matched.extend(relation.matched);
            result.censored.extend(relation.censored);
            sensitive |= order_sensitive;
        }
        return Ok((result, sensitive));
    }
    if !left.boss_spans.is_empty() && left.boss_spans.len() == right.boss_spans.len() {
        let mut result = Correspondence::default();
        let mut sensitive = false;
        for (&(left_start, left_end), &(right_start, right_end)) in
            left.boss_spans.iter().zip(&right.boss_spans)
        {
            let mut a = left.clone();
            let mut b = right.clone();
            for (pull, start, end) in [
                (&mut a, left_start, left_end),
                (&mut b, right_start, right_end),
            ] {
                pull.occurrences
                    .retain(|row| start <= row.relative_ms && row.relative_ms <= end);
                // Bound correspondence to this boss, e.g. movement cannot align a later boss's helper.
                pull.end_ms = end;
            }
            let (mut relation, order_sensitive) = path_correspondence(&a, &b)?;
            let a_signals = alignment::signals(&a);
            let b_signals = alignment::signals(&b);
            let defeated = boss_defeat(&b, right_start, right_end).unwrap_or(right_end);
            let opening = |signals: &[Signal]| {
                signals
                    .iter()
                    .find(|signal| signal.key.kind == "cast" && signal.key.role.ends_with("/Boss"))
                    .map(|signal| signal.time_ms)
            };
            if let (Some(a_entry), Some(b_entry)) = (opening(&a_signals), opening(&b_signals)) {
                // Bound repeated IDs before LCS can reuse an earlier round; allow observed conditional cast delays.
                // shortcut: the 10s bound covers these boss paths; replay longer-delay holdouts before widening it.
                relation.matched.retain(|source, target| {
                    a_signals
                        .iter()
                        .find(|s| s.event_indices.first() == Some(source))
                        .zip(
                            b_signals
                                .iter()
                                .find(|s| s.event_indices.first() == Some(target)),
                        )
                        .is_none_or(|(a, b)| {
                            !a.key.role.ends_with("/Boss")
                                || (a.time_ms - a_entry).abs_diff(b.time_ms - b_entry) <= 10000
                        })
                });
                // Resolve repeat ties only with unique observed timing, e.g. beam order cannot shift Eyes on Me to a later round.
                for signal in &a_signals {
                    let candidates: Vec<_> = b_signals
                        .iter()
                        .filter(|other| {
                            signal.key == other.key
                                && (signal.time_ms - a_entry).abs_diff(other.time_ms - b_entry)
                                    <= 2500
                        })
                        .collect();
                    if let [other] = candidates.as_slice()
                        && a_signals
                            .iter()
                            .filter(|candidate| {
                                candidate.key == other.key
                                    && (candidate.time_ms - a_entry)
                                        .abs_diff(other.time_ms - b_entry)
                                        <= 2500
                            })
                            .count()
                            == 1
                    {
                        let source = index(signal)?;
                        let target = index(other)?;
                        relation
                            .matched
                            .retain(|&old, mapped| old == source || *mapped != target);
                        relation.matched.insert(source, target);
                        relation.censored.remove(&source);
                    }
                }
                // Follow observed clock corrections, e.g. a delayed common cast also delays its missing suffix.
                let anchors: Vec<_> = a_signals
                    .iter()
                    .filter(|s| s.key.kind == "cast" && s.key.role.ends_with("/Boss"))
                    .filter_map(|s| {
                        relation
                            .matched
                            .get(&index(s).ok()?)
                            .and_then(|target| {
                                b_signals
                                    .iter()
                                    .find(|other| other.event_indices.first() == Some(target))
                            })
                            .map(|other| (s.time_ms, other.time_ms))
                    })
                    .collect();
                let expected_at = |signal: &Signal| {
                    let anchor = anchors
                        .iter()
                        .copied()
                        .filter(|&(at, _)| at <= signal.time_ms)
                        .max_by_key(|&(at, _)| at)
                        .unwrap_or((a_entry, b_entry));
                    signal.time_ms - anchor.0 + anchor.1
                };
                for signal in a_signals
                    .iter()
                    .filter(|signal| signal.key.actor_name.is_some())
                {
                    let source = index(signal)?;
                    let expected = expected_at(signal);
                    relation.matched.remove(&source);
                    let candidates: Vec<_> = b_signals
                        .iter()
                        .filter(|other| {
                            signal.key.actor_name == other.key.actor_name
                                && signal.key.role == other.key.role
                                && signal.key.kind == other.key.kind
                                && signal.key.ability_id == other.key.ability_id
                                && expected.abs_diff(other.time_ms) <= 2500
                        })
                        .collect();
                    let nearest = candidates
                        .iter()
                        .map(|other| expected.abs_diff(other.time_ms))
                        .min();
                    let candidates: Vec<_> = candidates
                        .into_iter()
                        .filter(|other| Some(expected.abs_diff(other.time_ms)) == nearest)
                        .collect();
                    if let [other] = candidates.as_slice() {
                        let target = index(other)?;
                        relation.matched.retain(|_, mapped| *mapped != target);
                        relation.matched.insert(source, target);
                        relation.censored.remove(&source);
                    }
                }
                for signal in &a_signals {
                    let source = index(signal)?;
                    if relation.matched.contains_key(&source) {
                        continue;
                    }
                    // Prefer a matched cast start, e.g. a helper begins late and its completion falls after the kill.
                    let expected =
                        a.occurrences
                            .iter()
                            .find(|row| row.event_index == source)
                            .and_then(|row| {
                                let start = a.occurrences.iter().find(|start| {
                                    Some(start.event_index) == row.start_event_index
                                })?;
                                let target = relation.matched.get(&start.event_index)?;
                                let observed = b
                                    .occurrences
                                    .iter()
                                    .find(|row| row.event_index == *target)?;
                                Some(signal.time_ms - start.relative_ms + observed.relative_ms)
                            })
                            .unwrap_or_else(|| expected_at(signal));
                    // Censor after the boss span, e.g. damage after a missing cast proves the boss was still alive.
                    if expected > defeated {
                        relation.censored.insert(source);
                    }
                }
            }
            result.matched.extend(relation.matched);
            result.censored.extend(relation.censored);
            sensitive |= order_sensitive;
        }
        return Ok((result, sensitive));
    }
    path_correspondence(left, right)
}

fn path_correspondence(left: &Pull, right: &Pull) -> Result<(Correspondence, bool)> {
    let comparison = alignment::compare(left, right)?;
    let mut result = Correspondence::default();
    let (mut left_gap, mut right_gap) = (Vec::new(), Vec::new());
    let mut alternative_matches = Vec::new();
    let mut unresolved_gap = false;
    for slot in comparison
        .segments
        .iter()
        .flat_map(|segment| &segment.slots)
    {
        match (&slot.left, &slot.right) {
            (Some(a), Some(b)) => {
                // A common observed successor is required; an unmatched final tail cannot prove an alternative.
                if alternatives(&left_gap, &right_gap) {
                    for (a, b) in left_gap.iter().zip(&right_gap) {
                        alternative_matches.push((*a, *b));
                    }
                } else if !left_gap.is_empty() || !right_gap.is_empty() {
                    unresolved_gap = true;
                }
                left_gap.clear();
                right_gap.clear();
                result.matched.insert(index(a)?, index(b)?);
            }
            (Some(a), None) => {
                if slot.evidence == "rightUnobservedAfterWipe" {
                    result.censored.insert(index(a)?);
                } else {
                    left_gap.push(a);
                }
            }
            (None, Some(b)) if slot.evidence != "leftUnobservedAfterWipe" => right_gap.push(b),
            _ => {}
        }
    }
    unresolved_gap |= !left_gap.is_empty() || !right_gap.is_empty();
    let changes: BTreeSet<_> = alternative_matches
        .iter()
        .map(|(a, b)| {
            (
                a.key.actor_game_id,
                &a.key.actor_name,
                &a.key.role,
                a.key.ability_id,
                b.key.ability_id,
            )
        })
        .collect();
    // A shared intermediate cast alone cannot prove independence: X,U,A → Y,U,B stays unresolved.
    // Only an isolated substitution with otherwise common observed paths gets an ID array.
    if !unresolved_gap && changes.len() == 1 {
        for (a, b) in alternative_matches {
            result.matched.insert(index(a)?, index(b)?);
        }
    }
    if comparison.order_sensitive {
        let reversed = alignment::compare(right, left)?;
        let reverse_matches: BTreeMap<_, _> = reversed
            .segments
            .iter()
            .flat_map(|s| &s.slots)
            .filter_map(|slot| {
                Some((
                    index(slot.right.as_ref()?).ok()?,
                    index(slot.left.as_ref()?).ok()?,
                ))
            })
            .collect();
        // Keep only direction-independent exact matches when repeated IDs admit different alignments.
        result
            .matched
            .retain(|a, b| reverse_matches.get(a) == Some(b));
    }
    Ok((result, comparison.order_sensitive))
}

pub(super) fn statistics(mut times: Vec<i64>) -> Result<TimeStatistics> {
    ensure!(!times.is_empty(), "No timing samples");
    times.sort();
    let middle = times.len() / 2;
    let hi = *times.get(middle).context("Missing median")?;
    let median = if times.len().is_multiple_of(2) {
        let lo = *times.get(middle - 1).context("Missing lower median")?;
        lo as f64 + (hi - lo) as f64 / 2.0
    } else {
        hi as f64
    };
    Ok(TimeStatistics {
        median_ms: median,
        min_ms: *times.first().context("Missing minimum")?,
        max_ms: *times.last().context("Missing maximum")?,
        sample_count: times.len(),
    })
}

pub(super) struct Input {
    pub pull: Pull,
    pub log: CollectedLog,
    pub report: Value,
    pub signals: BTreeMap<usize, Signal>,
    pub signal_indices: BTreeMap<usize, usize>,
    pub catalog: Vec<Ability>,
    pub actors: BTreeMap<i64, String>,
}

fn prepare(file: &str, mode: GenerateMode) -> Result<Input> {
    let source = load_one(&PathBuf::from_slash(file))?;
    let draft::SingleDraft {
        catalog, report, ..
    } = draft::build_single(&source, mode)?;
    // Align the same normalized read used for raw collision checks, e.g. if a pull was recollected.
    let mut pull = source.pull;
    if mode != GenerateMode::Raid {
        input::filter_boss_spans(&mut pull, &report)?;
    }
    let log = source.log;
    let actors: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.clone()))
        .collect();
    let signals = if mode == GenerateMode::Raid {
        wire_signals(&pull, &log, &actors)?
    } else {
        alignment::signals(&pull)
            .into_iter()
            .map(|signal| Ok((index(&signal)?, signal)))
            .collect::<Result<_>>()?
    };
    let signal_indices = signals
        .iter()
        .flat_map(|(&index, signal)| {
            signal
                .event_indices
                .iter()
                .map(move |&event| (event, index))
        })
        .collect();
    Ok(Input {
        pull,
        log,
        report,
        signals,
        signal_indices,
        catalog,
        actors,
    })
}

// Generation and evidence must agree on one wire cast, e.g. helper and boss copies share a representative.
pub(super) fn wire_signals(
    pull: &Pull,
    log: &CollectedLog,
    actors: &BTreeMap<i64, String>,
) -> Result<BTreeMap<usize, Signal>> {
    let mut waves: BTreeMap<_, Signal> = BTreeMap::new();
    for mut signal in alignment::signals(pull) {
        let source = log
            .events
            .get(index(&signal)?)
            .and_then(|event| event.source_id)
            .and_then(|id| actors.get(&id))
            .context("Missing signal source")?;
        let name = log
            .report
            .master_data
            .abilities
            .iter()
            .find(|ability| ability.game_id == signal.key.ability_id)
            .map(|ability| ability.name.clone())
            .context("Missing signal ability")?;
        let start = |signal: &Signal| {
            signal
                .event_indices
                .iter()
                .filter_map(|index| {
                    pull.occurrences
                        .iter()
                        .find(|row| row.event_index == *index)
                        .and_then(|row| row.start_timestamp_ms)
                })
                .min()
        };
        let wave_key = waves
            .iter()
            .find_map(|((wave, kind, other_name, other_source), other)| {
                (kind == &signal.key.kind
                    && other_name == &name
                    && other_source == source
                    && signal.time_ms.abs_diff(other.time_ms) <= 2500
                    && match (start(&signal), start(other)) {
                        (Some(a), Some(b)) => {
                            a.abs_diff(b) <= 750
                                && (signal.time_ms - other.time_ms).abs_diff(a - b) <= 500
                                && (signal.key.actor_game_id != other.key.actor_game_id
                                    || signal.key.role != other.key.role
                                    || (!signal.instance_ids.is_empty()
                                        && signal.instance_ids.iter().all(|instance| {
                                            !other.instance_ids.contains(instance)
                                        })))
                        }
                        (Some(_), None) | (None, Some(_)) => {
                            // Pairing consumes a start once; a late duplicate completion can be orphaned.
                            signal.time_ms.abs_diff(other.time_ms) <= 750
                                && !pull.occurrences.iter().any(|row| {
                                    row.kind == "begincast"
                                        && [signal.key.ability_id, other.key.ability_id]
                                            .contains(&row.ability_id)
                                        && actors.get(&row.actor_id) == Some(source)
                                        && row.relative_ms > signal.time_ms.min(other.time_ms)
                                        && row.relative_ms <= signal.time_ms.max(other.time_ms)
                                })
                        }
                        (None, None) => {
                            signal.key.kind == "begincast"
                                && signal.time_ms.abs_diff(other.time_ms) <= 750
                                && (signal.key.actor_game_id != other.key.actor_game_id
                                    || signal.key.role != other.key.role
                                    || (!signal.instance_ids.is_empty()
                                        && signal.instance_ids.iter().all(|instance| {
                                            !other.instance_ids.contains(instance)
                                        })))
                        }
                    })
                .then_some(*wave)
            })
            // Rejected merges keep unique keys, e.g. separate starts can complete in the same packet wave.
            .unwrap_or((signal.wave_ms, index(&signal)?));
        let key = (wave_key, signal.key.kind.clone(), name, source.clone());
        if let Some(wave) = waves.get_mut(&key) {
            // One wire event can have boss/helper copies, e.g. Kefka's All Things Ending.
            let count = wave.key.count + signal.key.count;
            if signal.key.role.ends_with("/Boss") {
                wave.key = signal.key;
            }
            wave.key.count = count;
            wave.event_indices.append(&mut signal.event_indices);
            wave.event_indices
                .sort_by_key(|&index| log.events.get(index).map(|event| (event.timestamp, index)));
            wave.time_ms = wave.time_ms.min(signal.time_ms);
            wave.instance_ids.extend(signal.instance_ids);
            wave.instance_ids.sort();
            wave.instance_ids.dedup();
            wave.key.instances = wave.instance_ids.len().max(1);
        } else {
            waves.insert(key, signal);
        }
    }
    let signals: BTreeMap<usize, Signal> = waves
        .into_values()
        .map(|signal| Ok((index(&signal)?, signal)))
        .collect::<Result<_>>()?;
    Ok(signals)
}

struct Consensus<'a> {
    samples: Vec<(usize, &'a Signal)>,
    censored: Vec<usize>,
}

// Keep relation indices on the merged wire wave, e.g. either Kefka copy denotes the same emitted row.
fn wire_correspondence(
    mut relation: Correspondence,
    left: &Input,
    right: &Input,
) -> Correspondence {
    let mut matched: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (a, b) in relation.matched {
        if let Some((&a, &b)) = left
            .signal_indices
            .get(&a)
            .zip(right.signal_indices.get(&b))
        {
            matched.entry(a).or_default().insert(b);
        }
    }
    relation.matched = matched
        .into_iter()
        .filter_map(|(index, targets)| {
            (targets.len() == 1)
                .then(|| targets.first().map(|&target| (index, target)))
                .flatten()
        })
        .collect();
    relation.censored = relation
        .censored
        .iter()
        .filter_map(|index| left.signal_indices.get(index).copied())
        .collect();
    relation
}

// Require all observed pairs to agree, e.g. a repeated cast must occupy the same position in every pull.
fn consensus_samples<'a>(
    inputs: &'a [Input],
    relations: &BTreeMap<(usize, usize), Correspondence>,
    reference: usize,
    event_index: usize,
) -> Result<Option<Consensus<'a>>> {
    let mut samples = Vec::new();
    let mut censored = Vec::new();
    let mut valid = true;
    for (i, input) in inputs.iter().enumerate() {
        let matched = if i == reference {
            Some(event_index)
        } else {
            relations
                .get(&(reference, i))
                .and_then(|relation| relation.matched.get(&event_index))
                .copied()
        };
        if let Some(index) = matched {
            samples.push((
                i,
                input
                    .signals
                    .get(&index)
                    .context("Missing aligned signal")?,
            ));
        } else if relations
            .get(&(reference, i))
            .is_some_and(|relation| relation.censored.contains(&event_index))
        {
            censored.push(i);
        } else {
            valid = false;
        }
    }
    for &(a, left) in &samples {
        let left_index = index(left)?;
        for &missing in &censored {
            if !relations
                .get(&(a, missing))
                .is_some_and(|relation| relation.censored.contains(&left_index))
            {
                valid = false;
            }
        }
    }
    for [&(a, left), &(b, right)] in samples.iter().array_combinations() {
        let left_index = index(left)?;
        let right_index = index(right)?;
        if !relations
            .get(&(a, b))
            .is_some_and(|relation| relation.matched.get(&left_index) == Some(&right_index))
        {
            valid = false;
        }
    }
    Ok(valid.then_some(Consensus { samples, censored }))
}

// Check raw casts before enabling the sync, e.g. an omitted helper can still activate a boss row.
fn cast_sync<'a>(
    inputs: &'a [Input],
    samples: &[(usize, &Signal)],
    ids: &BTreeSet<i64>,
    sources: &BTreeSet<String>,
    at: f64,
    slot: usize,
) -> Result<(NetworkSync, Option<MultiConflict<'a>>)> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "id".into(),
        field_pattern(ids.iter().map(|id| format!("{id:X}")).collect())?,
    );
    let invalid_source = sources
        .iter()
        .any(|name| name.contains(['#', '"', '\r', '\n']));
    if !invalid_source {
        let patterns = sources
            .iter()
            .map(|source| regress::escape(source))
            .collect();
        fields.insert("source".into(), field_pattern(patterns)?);
    }
    let mut collisions = Vec::new();
    let mut outside_window = false;
    for (i, input) in inputs.iter().enumerate() {
        let sample = samples
            .iter()
            .find(|&&(position, _)| position == i)
            .map(|&(_, sample)| sample);
        let start = input
            .log
            .report
            .fights
            .first()
            .context("Missing fight")?
            .start_time;
        let representative = sample.map(index).transpose()?;
        let sample_outside_window = sample.is_some_and(|s| {
            (s.time_ms as f64) < at * 1000.0 - 2500.0 || (s.time_ms as f64) >= at * 1000.0 + 2500.0
        });
        outside_window |= sample_outside_window;
        let observed_ms =
            sample.map_or(at * 1000.0, |s| draft::rounded_seconds(s.time_ms) * 1000.0);
        for (event_index, event) in input.log.events.iter().enumerate() {
            let source_matches = event
                .source_id
                .and_then(|actor| input.actors.get(&actor))
                .is_some_and(|name| sources.contains(name));
            if event.kind == "cast"
                && event.ability_game_id.is_some_and(|id| ids.contains(&id))
                && source_matches
                && Some(event_index) != representative
                // A boss's merged copies belong to this cast, e.g. its main and helper completion IDs share one row.
                && !sample.is_some_and(|signal| signal.key.role.ends_with("/Boss")
                    && signal.event_indices.contains(&event_index))
                // A drift-disabled draft needs clock correction first, e.g. an 8s shift between pulls.
                && ((!sample_outside_window
                    && ((event.timestamp - start) as f64 - at * 1000.0).abs() <= 2500.0)
                    || ((event.timestamp - start) as f64 - observed_ms).abs() <= 2500.0)
            {
                collisions.push(ConflictingEvent {
                    file: &input.pull.file,
                    event_index,
                });
            }
        }
    }
    let reason = if invalid_source {
        Some("source name cannot be rendered safely as a sync")
    } else if !collisions.is_empty() {
        Some("another cast matches within the default sync window")
    } else if outside_window {
        Some("observed cast falls outside the draft sync window")
    } else {
        None
    };
    Ok((
        NetworkSync {
            log: LogType::Ability,
            fields,
            enabled: reason.is_none(),
            window: None,
        },
        reason.map(|reason| MultiConflict {
            slot,
            reason,
            conflicting_events: collisions,
        }),
    ))
}

// Trace coverage to raw indices, e.g. collapsed instances all count as represented by one row.
fn output_coverage<'a>(input: &'a Input, slots: &[MultiSlot<'_>]) -> OutputCoverage<'a> {
    let file = &input.pull.file;
    let mut represented = BTreeSet::new();
    for slot in slots {
        for sample in &slot.samples {
            if sample.file == file {
                represented.extend(sample.event_indices.iter().copied());
            }
        }
    }
    let omitted_events = input
        .signals
        .values()
        .filter(|signal| signal.key.kind == "cast")
        .flat_map(|signal| &signal.event_indices)
        .filter(|&&index| !represented.contains(&index))
        .copied()
        .collect();
    OutputCoverage {
        file,
        represented_event_indices: represented,
        omitted_event_indices: omitted_events,
    }
}

pub(super) fn build(group: Group, mode: GenerateMode, lookahead: f64) -> Result<(String, Value)> {
    let Group { key, mut pulls } = group;
    pulls.sort_by(|a, b| (&a.report, a.fight, &a.file).cmp(&(&b.report, b.fight, &b.file)));
    // Own each pull beside its raw evidence; sorting/filtering cannot desynchronize parallel lists.
    let inputs = pulls
        .into_par_iter()
        .map(|pull| prepare(&pull.file, mode))
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    // Build fixed phase schedules before mining divergent paths, e.g. randomized IDs need not fork a fixed clock.
    if mode == GenerateMode::Raid
        && let Some(schedule) = super::schedule::build(&key, &inputs, lookahead)?
    {
        return Ok(schedule);
    }
    // The longest observed path provides positions, never permission to emit its exclusive branches.
    let reference = inputs
        .iter()
        .enumerate()
        .max_by_key(|(i, input)| (input.pull.occurrences.len(), std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
        .context("Missing reference pull")?;
    let mut relations = BTreeMap::new();
    let mut sensitive = Vec::new();
    // ponytail: reuse P4 pair evidence; an indexed graph is appropriate only for substantially larger groups.
    // Index pairs before parallel work so report ordering stays stable, e.g. (0,1) precedes (0,2).
    let pairs: Vec<_> = inputs.iter().enumerate().array_combinations().collect();
    let comparisons: Vec<_> = pairs
        .par_iter()
        .map(|[(_, left), (_, right)]| {
            let (forward, order_sensitive) = correspondence(&left.pull, &right.pull)?;
            let (backward, _) = correspondence(&right.pull, &left.pull)?;
            Ok::<_, anyhow::Error>((
                wire_correspondence(forward, left, right),
                wire_correspondence(backward, right, left),
                order_sensitive,
            ))
        })
        .progress_with_style(ProgressStyle::with_template(
            "Aligning pulls {wide_bar} {pos}/{len}",
        )?)
        .collect();
    for (pair, comparison) in pairs.iter().zip(comparisons) {
        let [(a, left), (b, right)] = *pair;
        let (forward, backward, order_sensitive) = comparison?;
        relations.insert((a, b), forward);
        relations.insert((b, a), backward);
        if order_sensitive {
            sensitive.push(SensitivePair {
                left: &left.pull.file,
                right: &right.pull.file,
            });
        }
    }
    let reference_signals = &inputs
        .get(reference)
        .context("Missing reference input")?
        .signals;
    let mut entries = vec![Entry::Note {
        text: format!(
            "Draft from {} compatible FFLogs pulls; common rows and alternatives with an observed common successor. Repeats are finite; unresolved paths are omitted. No encounter transitions or replay validation.",
            inputs.len()
        ),
    }];
    let mut slots = Vec::new();
    let mut omitted = Vec::new();
    let mut conflicts = Vec::new();
    let mut catalog = BTreeMap::new();
    for input in &inputs {
        for entry in input
            .report
            .get("abilityNames")
            .context("Missing ability names")?
            .as_object()
            .context("Missing ability names")?
        {
            catalog
                .entry(entry.0.clone())
                .or_insert_with(|| entry.1.clone());
        }
    }
    let mut blocks = vec![MultiBlock {
        id: 0,
        entry: "fightStart",
        draft_entry_ms: 0.into(),
        time: statistics(vec![0; inputs.len()])?,
        slot: None,
        samples: None,
    }];
    let mut anchor_indices: Option<Vec<Option<usize>>> = None;
    let mut block_id = 0;
    let mut previous_at = 0.0_f64;
    let mut boss_section = None;
    for (&event_index, signal) in reference_signals
        .iter()
        .sorted_by_key(|(index, signal)| (alignment::signal_order(signal), **index))
    {
        if mode != GenerateMode::Raid {
            let section = inputs
                .get(reference)
                .context("Missing reference input")?
                .pull
                .boss_spans
                .iter()
                .position(|&(lo, hi)| lo <= signal.time_ms && signal.time_ms <= hi);
            if section != boss_section {
                // A new boss starts its own anchors, e.g. a slow Eye clear must not hide Chort's opening.
                anchor_indices = None;
                boss_section = section;
            }
        }
        let Some(Consensus { samples, censored }) =
            consensus_samples(&inputs, &relations, reference, event_index)?
        else {
            omitted.push(OmittedSignal {
                file: Some(
                    &inputs
                        .get(reference)
                        .context("Missing reference")?
                        .pull
                        .file,
                ),
                event_indices: &signal.event_indices,
                reason: "no direction-independent group consensus or dependent/unresolved path",
                evidence: "unknown",
                samples: None,
                time: None,
            });
            continue;
        };
        if signal.key.kind != "cast" {
            continue;
        }
        let mut relative_times = Vec::new();
        let mut source_refs = Vec::new();
        let mut ids = BTreeSet::new();
        let mut sources = BTreeSet::new();
        let mut block_observed = true;
        for &(i, sample) in &samples {
            let input = inputs.get(i).context("Missing sample input")?;
            let entry_ms = match &anchor_indices {
                Some(anchors) => anchors
                    .get(i)
                    .copied()
                    .flatten()
                    .and_then(|index| input.signals.get(&index))
                    .map(|s| s.time_ms),
                None => Some(0),
            };
            let Some(entry_ms) = entry_ms else {
                block_observed = false;
                continue;
            };
            relative_times.push(sample.time_ms - entry_ms);
            ids.extend(sample.event_indices.iter().filter_map(|&index| {
                input
                    .log
                    .events
                    .get(index)
                    .and_then(|event| event.ability_game_id)
            }));
            for &event in &sample.event_indices {
                let raw = input
                    .log
                    .events
                    .get(event)
                    .context("Missing source event")?;
                let actor = raw.source_id.context("Missing source actor")?;
                sources.insert(
                    input
                        .actors
                        .get(&actor)
                        .context("Missing source name")?
                        .clone(),
                );
            }
            source_refs.push(Sample {
                file: &input.pull.file,
                event_indices: &sample.event_indices,
                instance_ids: &sample.instance_ids,
                ability_id: sample.key.ability_id,
                time_ms: sample.time_ms,
                block_entry_ms: entry_ms,
                relative_ms: sample.time_ms - entry_ms,
            });
        }
        if !block_observed {
            omitted.push(OmittedSignal {
                file: None,
                event_indices: &signal.event_indices,
                reason: "block entry unobserved",
                evidence: "unknown",
                samples: None,
                time: None,
            });
            continue;
        }
        let timing = statistics(relative_times)?;
        let absolute = statistics(samples.iter().map(|(_, s)| s.time_ms).collect())?;
        // Raid medians preserve shared order; dungeon placeholders follow the reference until boss clocks are compiled.
        // A slow Eye suffix must not move ahead of a fast Chort opening just because the reached sample set changed.
        let at_ms = if mode == GenerateMode::Raid {
            absolute.median_ms
        } else {
            signal.time_ms as f64
        };
        let mut at = (at_ms / 100.0).round() / 10.0;
        if at < previous_at && previous_at - at <= 0.25 {
            // Keep a single packet wave on one clock tick, e.g. a helper precedes its caster by 45ms.
            at = previous_at;
        }
        if at < previous_at {
            // A changing reached-pull subset can invert medians even though each observed path is ordered.
            omitted.push(OmittedSignal {
                file: None,
                event_indices: &signal.event_indices,
                samples: Some(source_refs),
                time: Some(timing),
                reason: "timing medians violate order after the reached sample set changes",
                evidence: "unknown",
            });
            continue;
        }
        previous_at = at;
        let name = ids
            .iter()
            .map(|id| {
                catalog
                    .get(&id.to_string())
                    .and_then(Value::as_str)
                    .context("Missing catalog name")
            })
            .process_results(|names| names.unique().join(" / "))?;
        let (sync, conflict) = cast_sync(&inputs, &samples, &ids, &sources, at, slots.len())?;
        let reason = conflict.as_ref().map(|conflict| conflict.reason);
        if let Some(conflict) = conflict {
            conflicts.push(conflict);
        }
        entries.push(Entry::Event {
            at,
            name,
            duration: None,
            jump: None,
            sync: Some(Sync::Network(sync)),
            note: reason.map(|reason| {
                format!(
                    "Sync disabled: {reason}; see slot {} in report",
                    slots.len()
                )
            }),
        });
        slots.push(MultiSlot {
            id: slots.len(),
            block: block_id,
            at_ms,
            time: timing,
            absolute_time: absolute,
            ability_ids: ids.iter().copied().collect(),
            samples: source_refs.clone(),
            unobserved_after_wipe: censored
                .iter()
                .filter_map(|&i| inputs.get(i).map(|input| input.pull.file.as_str()))
                .collect(),
            evidence: if ids.len() > 1 {
                "alternativeWithCommonSuccessor"
            } else {
                "observed"
            },
        });
        if signal.key.role.ends_with("/Boss") && ids.len() == 1 {
            block_id = blocks.len();
            let mut anchors = vec![None; inputs.len()];
            for &(i, sample) in &samples {
                *anchors.get_mut(i).context("Missing anchor position")? = Some(index(sample)?);
            }
            anchor_indices = Some(anchors);
            blocks.push(MultiBlock {
                id: block_id,
                entry: "observedBossCast",
                slot: Some(slots.len() - 1),
                draft_entry_ms: serde_json::Number::from_f64(at_ms)
                    .context("Invalid block time")?,
                time: absolute,
                samples: Some(source_refs),
            });
        }
    }
    // Reuse P3's mode-filtered first-seen catalogs so unfinished casts keep their provenance too.
    let abilities = inputs
        .iter()
        .flat_map(|input| &input.catalog)
        .unique_by(|ability| &ability.id)
        .cloned()
        .collect();
    entries.push(Entry::AbilityCatalog {
        abilities,
        phase: None,
    });
    let output_coverage = inputs
        .iter()
        .map(|input| output_coverage(input, &slots))
        .collect();
    let mut unordered = BTreeSet::new();
    for (i, a) in slots.iter().enumerate() {
        for (j, b) in slots
            .iter()
            .enumerate()
            .skip(i + 1)
            .take_while(|(_, b)| b.at_ms - a.at_ms <= 250.0)
        {
            let differences: Vec<_> = a
                .samples
                .iter()
                .filter_map(|sample| {
                    b.samples
                        .iter()
                        .find(|other| other.file == sample.file)
                        .map(|other| sample.time_ms - other.time_ms)
                })
                .collect();
            if differences.iter().all(|time| time.abs_diff(0) <= 250)
                && differences.iter().any(|&time| time > 0)
            {
                // Preserve measured packet order swaps even in the common draft, e.g. two Exdeath copies.
                unordered.extend([i, j]);
            }
        }
    }
    for i in unordered {
        if let Some(slot) = slots.get_mut(i) {
            slot.evidence = "unorderedPacketWave";
        }
    }
    let yaml = draft::serialize_draft(entries, mode.reset_on())?;
    let report = serde_json::to_value(MultiReport {
        status: "draft",
        mode,
        group: key,
        inputs: inputs.iter().map(|input| &input.report).collect(),
        blocks,
        slots,
        sync_conflicts: conflicts,
        omitted_signals: omitted,
        output_coverage,
        observed_paths: inputs
            .iter()
            .map(|input| ObservedPath {
                file: &input.pull.file,
                occurrences: &input.pull.occurrences,
            })
            .collect(),
        unobserved_combinations: "unknown",
        alignment: Alignment {
            implementation: "similar 3 Myers/LCS",
            order_sensitive_pairs: sensitive,
        },
        limitations: [
            "Unresolved or dependent paths are omitted; unobserved combinations remain unknown.",
            "ID arrays require an isolated one-ability substitution and an observed common successor; multiple varying choices remain unresolved.",
            "Repeats are expanded finitely; no conditional exits or encounter transitions are inferred.",
            "Block entries are observational timing references; entry sync, replay and runtime compatibility are unverified.",
        ],
        validation: Validation::default(),
    })?;
    let (yaml, report) = super::phase::expand(yaml, report, &inputs, lookahead)?;
    let (yaml, report) = super::repeat::compress(yaml, report, &inputs, lookahead)?;
    let (yaml, mut report) = super::sections::separate(yaml, report, lookahead)?;
    super::sections::check(
        &yaml,
        &mut report,
        &inputs
            .iter()
            .map(|input| (&input.pull, &input.log))
            .collect::<Vec<_>>(),
    )?;
    Ok((yaml, report))
}

// Preserve scalar patterns for one value and arrays for alternatives, e.g. A versus [A, B].
pub(super) fn field_pattern(mut patterns: Vec<String>) -> Result<FieldPattern> {
    if patterns.len() == 1 {
        Ok(FieldPattern::One(
            patterns.pop().context("Missing field pattern")?,
        ))
    } else {
        Ok(FieldPattern::Many(patterns))
    }
}

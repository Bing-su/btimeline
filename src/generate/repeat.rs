use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::alignment::{self, Signal, SignalKey};
use super::multi::{Input, statistics};
use super::phase::{put, sample_index};
use super::{Pull, draft, replay};
use crate::fflogs::model::CollectedLog;
use crate::timeline::{Destination, Entry, Jump, JumpWhen, Sync, Timeline};

// Compare observable context, e.g. identical ability IDs on different helper instances are distinct.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CastContext {
    key: SignalKey,
    instance_ids: Vec<i64>,
    paired_start: bool,
    targetability: Vec<Value>,
}

pub(super) fn context(pull: &Pull, log: &CollectedLog, signal: &Signal) -> CastContext {
    let mut state = BTreeMap::new();
    // Use source order only to break equal-time ties; FFLogs input need not already be sorted.
    let start = log
        .report
        .fights
        .first()
        .map_or(0, |fight| fight.start_time);
    let mut updates: Vec<_> = log
        .events
        .iter()
        .enumerate()
        .filter(|(index, event)| {
            event.kind == "targetabilityupdate"
                && signal.event_indices.first().is_some_and(|cast_index| {
                    (event.timestamp, *index) < (start + signal.time_ms, *cast_index)
                })
        })
        .collect();
    updates.sort_by_key(|(index, event)| (event.timestamp, *index));
    for (_, event) in updates {
        if let Some(actor) = event.target_id.or(event.source_id)
            && let Some(identity) = pull.actors.get(&actor)
        {
            state.insert(
                (identity, event.source_instance),
                event.extra.get("targetable").cloned(),
            );
        }
    }
    CastContext {
        key: signal.key.clone(),
        instance_ids: signal.instance_ids.clone(),
        paired_start: signal.event_indices.iter().all(|index| {
            pull.occurrences.iter().any(|row| row.event_index == *index && row.start_event_index.is_some())
        }),
        targetability: state.into_iter().map(|((actor, instance), value)| {
            json!({"actorGameId": actor.0, "role": actor.1, "instance": instance, "targetable": value})
        }).collect(),
    }
}

fn event_time(entry: &Entry) -> Option<i64> {
    match entry {
        Entry::Event { at, .. } | Entry::Label { at, .. } => Some((at * 1000.0).round() as i64),
        _ => None,
    }
}

fn jump(entry: &mut Entry, label: &str) -> Result<()> {
    let Entry::Event {
        sync: Some(Sync::Network(sync)),
        jump,
        ..
    } = entry
    else {
        anyhow::bail!("repeat requires a network sync");
    };
    ensure!(sync.enabled, "repeat requires an enabled sync");
    *jump = Some(Jump {
        to: Destination::Label(label.into()),
        when: JumpWhen::Sync,
    });
    Ok(())
}

// Fit corrected intervals instead of summing median durations, e.g. each successful sync resets drift.
fn place(entry: &mut Entry, slot: &mut Value, clocks: Vec<i64>) -> Result<i64> {
    let timing = statistics(clocks)?;
    let at = (timing.median_ms / 100.0).round() as i64 * 100;
    let before = (((at - timing.min_ms).max(0) + 99) / 100 * 100).max(2500);
    // Keep the latest observation strictly inside the window, e.g. +5s needs an upper edge at 5.1s.
    let after = (((timing.max_ms - at).max(0) / 100 + 1) * 100).max(2500);
    let Entry::Event {
        at: entry_at,
        sync: Some(Sync::Network(sync)),
        ..
    } = entry
    else {
        anyhow::bail!("repeat requires an event sync");
    };
    ensure!(sync.enabled, "repeat contains a disabled sync");
    *entry_at = at as f64 / 1000.0;
    sync.window = Some([before as f64 / 1000.0, after as f64 / 1000.0]);
    slot["atMs"] = at.into();
    slot["clockTime"] = serde_json::to_value(timing)?;
    slot["windowMs"] = json!([before, after]);
    Ok(at)
}

// Keep finite evidence as the oracle; compression changes coordinates, never the occurrence contract.
pub(super) fn fold_evidence(
    yaml: &str,
    report: &Value,
    key: &super::GroupKey,
    pull: &Pull,
    log: &CollectedLog,
    peers: &BTreeMap<&str, &Pull>,
) -> Result<crate::timeline::replay::Evidence> {
    let repeats = &report["repeats"];
    ensure!(
        repeats["finiteEvidence"].get("repeats").is_none(),
        "Nested repeat evidence is unsupported"
    );
    let finite_yaml = repeats["finiteYaml"]
        .as_str()
        .context("Missing finite repeat timeline")?;
    let finite = replay::evidence(
        finite_yaml,
        &repeats["finiteEvidence"],
        key,
        pull,
        log,
        peers,
    )?;
    let timeline: Timeline = serde_saphyr::from_str(yaml)?;
    let original: Timeline = serde_saphyr::from_str(finite_yaml)?;
    let event_indices = |timeline: &Timeline| {
        timeline
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry, Entry::Event { .. }) && !entry.is_combat_start())
            .map(|(index, _)| index)
            .collect::<Vec<_>>()
    };
    let entries = event_indices(&timeline);
    let old_entries = event_indices(&original);
    let mapping: Vec<usize> = serde_json::from_value(repeats["slotMapping"].clone())?;
    ensure!(
        mapping.len() == old_entries.len()
            && mapping.iter().all(|slot| entries.get(*slot).is_some()),
        "Invalid repeat slot mapping"
    );
    let signals = alignment::signals(pull);
    let mut result = crate::timeline::replay::Evidence {
        lookahead_ms: Some(
            repeats["lookaheadMs"]
                .as_i64()
                .context("Missing repeat lookahead")?,
        ),
        ..Default::default()
    };
    for (new_slot, &entry) in entries.iter().enumerate() {
        let old: Vec<_> = mapping
            .iter()
            .enumerate()
            .filter(|(_, slot)| **slot == new_slot)
            .filter_map(|(index, _)| old_entries.get(index))
            .copied()
            .collect();
        ensure!(!old.is_empty(), "Repeat row has no finite evidence");
        let expected: BTreeSet<_> = old
            .iter()
            .filter_map(|index| finite.expected.get(index))
            .flatten()
            .copied()
            .collect();
        let unresolved = old.iter().any(|index| {
            !finite.expected.contains_key(index)
                && !finite.censored.contains(index)
                && !finite.inactive.contains(index)
        });
        if unresolved {
            // Combining rows cannot conceal an unresolved round, e.g. A,B,A versus A,A.
            result.missing.insert(entry);
        } else if !expected.is_empty() {
            result.expected.insert(entry, expected);
        } else if old.iter().all(|index| finite.censored.contains(index)) {
            result.censored.insert(entry);
        } else if old.iter().all(|index| finite.inactive.contains(index)) {
            result.inactive.insert(entry);
        } else if old.iter().any(|index| finite.missing.contains(index)) {
            result.missing.insert(entry);
        }
        // A new context must fail even if its ID/time matches, e.g. a holdout changes targetability.
        if let Some(expected_context) = report["slots"]
            .get(new_slot)
            .and_then(|slot| slot.get("repeatContext"))
        {
            let expected_context: CastContext = serde_json::from_value(expected_context.clone())?;
            if result.expected.get(&entry).is_some_and(|indices| {
                indices.iter().any(|index| {
                    signals
                        .iter()
                        .find(|signal| signal.event_indices.first() == Some(index))
                        .is_none_or(|signal| context(pull, log, signal) != expected_context)
                })
            }) {
                result.expected.remove(&entry);
                result.missing.insert(entry);
            }
        }
    }
    // Require every unresolved finite occurrence, e.g. an early exit cannot skip its continuation gap.
    result.required_missing = result.missing.clone();
    Ok(result)
}

pub(super) fn compress(
    yaml: String,
    mut report: Value,
    inputs: &[Input],
    lookahead: f64,
) -> Result<(String, Value)> {
    let reference = inputs
        .iter()
        .max_by_key(|input| input.pull.occurrences.len())
        .context("Missing repeat input")?;
    let mut casts: Vec<_> = reference
        .signals
        .values()
        .filter(|signal| signal.key.kind == "cast")
        .collect();
    casts.sort_by_key(|signal| (signal.time_ms, signal.event_indices.first().copied()));
    let mut candidates = Vec::new();
    let mut start = 0;
    // ponytail: scan adjacent primitive blocks; nested/overlapping loops need a graph only when required.
    while start < casts.len() {
        let width = (1..=(casts.len() - start) / 2).find(|&width| {
            casts
                .iter()
                .skip(start)
                .take(width)
                .zip(casts.iter().skip(start + width).take(width))
                .all(|(a, b)| {
                    a.key.actor_game_id == b.key.actor_game_id
                        && a.key.ability_id == b.key.ability_id
                })
        });
        let Some(width) = width else {
            start += 1;
            continue;
        };
        let mut rounds = 2;
        while start + (rounds + 1) * width <= casts.len()
            && casts
                .iter()
                .skip(start)
                .take(width)
                .zip(casts.iter().skip(start + rounds * width).take(width))
                .all(|(a, b)| {
                    a.key.actor_game_id == b.key.actor_game_id
                        && a.key.ability_id == b.key.ability_id
                })
        {
            rounds += 1;
        }
        candidates.push(
            json!({"referenceFile": reference.pull.file, "start": start, "width": width,
            "rounds": rounds, "accepted": false, "evaluated": false,
            "reason": "not evaluated; at most one independent loop is compiled per group",
            "roundEventIndices": (0..rounds).map(|round| casts.iter().skip(start + round * width).take(width)
                .flat_map(|signal| signal.event_indices.iter().copied()).collect::<Vec<_>>()).collect::<Vec<_>>(),
            "period": statistics((1..rounds).filter_map(|round| {
                Some(casts.get(start + round * width)?.time_ms - casts.get(start + (round - 1) * width)?.time_ms)
            }).collect())?,
            "exitEventIndices": casts.get(start + rounds * width).map(|signal| &signal.event_indices)}),
        );
        start += rounds * width;
    }
    if candidates.is_empty() {
        return Ok((yaml, report));
    }
    for candidate in &mut candidates {
        candidate["evaluated"] = true.into();
        let attempt = compile(
            &yaml, &report, inputs, reference, &casts, candidate, lookahead,
        );
        match attempt {
            Ok((compiled_yaml, mut compiled_report, checks)) => {
                candidate["accepted"] = true.into();
                candidate["reason"] =
                    "observed exit and independent raw-signal replay passed".into();
                let repeats = compiled_report
                    .get_mut("repeats")
                    .context("Missing repeat evidence")?;
                put(&candidates, repeats, "candidates")?;
                put(checks, repeats, "checks")?;
                return Ok((compiled_yaml, compiled_report));
            }
            Err(error) => candidate["reason"] = format!("{error:#}").into(),
        }
    }
    put(
        json!({"accepted": false, "lookaheadMs": (lookahead * 1000.0).ceil() as i64,
        "candidates": candidates, "policy": "finite rows retained without proven context, wipe/clear exit and raw replay"}),
        &mut report,
        "repeats",
    )?;
    Ok((yaml, report))
}

fn compile(
    yaml: &str,
    report: &Value,
    inputs: &[Input],
    reference: &Input,
    casts: &[&Signal],
    candidate: &mut Value,
    lookahead: f64,
) -> Result<(String, Value, Value)> {
    let start = candidate["start"]
        .as_u64()
        .context("Missing repeat start")? as usize;
    let width = candidate["width"]
        .as_u64()
        .context("Missing repeat width")? as usize;
    let rounds = candidate["rounds"]
        .as_u64()
        .context("Missing repeat rounds")? as usize;
    let end = start + rounds * width;
    let exit = casts
        .get(end)
        .context("no observed exit; fight termination is not an exit signal")?;
    let first = casts.get(start).context("Missing repeat opening")?;
    ensure!(
        casts
            .iter()
            .skip(start + 1)
            .take(width - 1)
            .all(|signal| signal.key.ability_id != first.key.ability_id),
        "repeat entry also occurs inside its body"
    );
    ensure!(
        casts
            .iter()
            .skip(start)
            .take(width)
            .all(|signal| signal.key.ability_id != exit.key.ability_id),
        "exit is not distinguishable from the repeat body"
    );
    let slots = report["slots"].as_array().context("Missing repeat slots")?;
    let positions: Vec<_> = casts
        .iter()
        .skip(start)
        .take(rounds * width + 1)
        .map(|signal| {
            slots
                .iter()
                .position(|slot| {
                    sample_index(slot, &reference.pull.file)
                        == signal.event_indices.first().copied()
                })
                .context("repeat or exit lacks direction-independent alignment")
        })
        .collect::<Result<_>>()?;
    let begin = *positions.first().context("Missing repeat position")?;
    ensure!(
        positions
            .iter()
            .enumerate()
            .all(|(offset, position)| *position == begin + offset),
        "repeat crosses an unresolved path"
    );
    let exit_slot = begin + rounds * width;
    let contexts = casts
        .iter()
        .skip(start)
        .take(width)
        .map(|signal| context(&reference.pull, &reference.log, signal))
        .collect::<Vec<_>>();
    let exit_context = context(&reference.pull, &reference.log, exit);
    let mut observations = Vec::new();
    let mut periods = Vec::new();
    let mut exit_offsets = Vec::new();
    let mut exit_kinds = BTreeSet::new();
    let mut exit_reports = BTreeSet::new();
    let mut round_sequence = None;
    let mut final_sequences = Vec::new();
    for input in inputs {
        let mut starts = Vec::new();
        let mut observed = Vec::new();
        for (offset, slot) in slots.iter().skip(begin).take(rounds * width).enumerate() {
            if let Some(index) = sample_index(slot, &input.pull.file) {
                let signal = input
                    .signals
                    .get(&index)
                    .context("Missing repeated signal")?;
                let expected_context = contexts
                    .get(offset % width)
                    .context("Missing repeat context")?;
                ensure!(
                    &context(&input.pull, &input.log, signal) == expected_context,
                    "role, instance, start/completion or targetability context differs"
                );
                if offset % width == 0 {
                    starts.push(signal.time_ms);
                }
                observed.push(index);
            }
        }
        // Include every normalized start/helper, e.g. an omitted begin-cast cannot create a false cycle.
        let mut sequences = Vec::new();
        for pair in starts.windows(2) {
            let [from, to] = pair else { continue };
            ensure!(to > from, "repeat has a nonpositive interval");
            periods.push(to - from);
            let mut sequence: Vec<_> = input
                .signals
                .values()
                .filter(|signal| signal.time_ms >= *from && signal.time_ms < *to)
                .collect();
            sequence.sort_by_key(|signal| (signal.time_ms, signal.event_indices.first().copied()));
            sequences.push(
                sequence
                    .into_iter()
                    .map(|signal| (&signal.key, &signal.instance_ids))
                    .collect::<Vec<_>>(),
            );
        }
        for sequence in &sequences {
            if let Some(expected) = &round_sequence {
                ensure!(
                    sequence == expected,
                    "start/helper order differs across pulls"
                );
            } else {
                round_sequence = Some(sequence.clone());
            }
        }
        ensure!(
            sequences
                .windows(2)
                .all(|pair| matches!(pair, [a, b] if a == b)),
            "start/helper order differs between rounds"
        );
        let exit_index = sample_index(
            slots.get(exit_slot).context("Missing exit slot")?,
            &input.pull.file,
        );
        // Include the final tail up to exit/termination; E's paired start belongs to the exit, not A,B.
        if let Some(from) = starts.last() {
            let boundary = exit_index
                .and_then(|index| input.signals.get(&index))
                .map(|signal| {
                    (
                        signal.time_ms,
                        signal.event_indices.first().copied().unwrap_or(0),
                    )
                })
                .unwrap_or((input.pull.end_ms, usize::MAX));
            let exit_starts: BTreeSet<_> = input
                .pull
                .occurrences
                .iter()
                .filter(|row| Some(row.event_index) == exit_index)
                .filter_map(|row| row.start_event_index)
                .collect();
            let mut sequence: Vec<_> = input
                .signals
                .values()
                .filter(|signal| {
                    signal.time_ms >= *from
                        && (
                            signal.time_ms,
                            signal.event_indices.first().copied().unwrap_or(0),
                        ) < boundary
                        && !signal
                            .event_indices
                            .iter()
                            .any(|index| exit_starts.contains(index))
                })
                .collect();
            sequence.sort_by_key(|signal| (signal.time_ms, signal.event_indices.first().copied()));
            final_sequences.push(
                sequence
                    .into_iter()
                    .map(|signal| (&signal.key, &signal.instance_ids))
                    .collect::<Vec<_>>(),
            );
        }
        if let Some(index) = exit_index {
            ensure!(
                observed.len() == rounds * width,
                "exit follows a different or incomplete repeat count"
            );
            let signal = input.signals.get(&index).context("Missing exit signal")?;
            ensure!(
                context(&input.pull, &input.log, signal) == exit_context,
                "exit context differs"
            );
            exit_offsets.push(signal.time_ms - starts.last().context("Missing last round")?);
            exit_kinds.insert(input.pull.kill);
            exit_reports.insert(&input.pull.report);
        }
        observations.push(json!({"file": input.pull.file, "termination": if input.pull.kill {"kill"} else {"wipe"},
            "roundStartsMs": starts, "eventIndices": observed, "exitEventIndex": exit_index}));
    }
    let expected_sequence = round_sequence.as_ref().context("Missing repeat sequence")?;
    ensure!(
        final_sequences
            .iter()
            .all(|sequence| expected_sequence.starts_with(sequence)),
        "start/helper order differs in the final round"
    );
    candidate["contexts"] = serde_json::to_value(&contexts)?;
    candidate["observations"] = serde_json::to_value(observations)?;
    candidate["period"] = serde_json::to_value(statistics(periods)?)?;
    ensure!(!exit_offsets.is_empty(), "no aligned exit observations");
    candidate["exitOffset"] = serde_json::to_value(statistics(exit_offsets)?)?;
    ensure!(
        exit_kinds.len() == 2 && exit_reports.len() >= 2,
        "exit requires reached wipe and clear from at least two reports"
    );
    ensure!(
        rounds * width > width + 1,
        "compression would not remove an event row"
    );
    let timeline: Timeline = serde_saphyr::from_str(yaml)?;
    // Keep P7 control flow intact: composing nested loops needs distinct occurrence evidence first.
    ensure!(
        !timeline.entries.iter().any(|entry| matches!(
            entry,
            Entry::Label { .. } | Entry::Event { jump: Some(_), .. }
        )),
        "repeat composition with existing branch/phase jumps remains finite"
    );
    let mut events = Vec::new();
    let mut other = Vec::new();
    for entry in timeline.entries {
        if entry.is_combat_start() {
            continue;
        }
        if matches!(entry, Entry::Event { .. }) {
            events.push(entry);
        } else {
            other.push(entry);
        }
    }
    ensure!(events.len() == slots.len(), "Repeat event count differs");
    let longest = inputs
        .iter()
        .map(|input| input.pull.end_ms)
        .max()
        .unwrap_or(0);
    let horizon = (lookahead * 1000.0).ceil() as i64;
    let guard = longest
        .checked_add(horizon)
        .and_then(|value| value.checked_add(2700))
        .context("Repeat clock overflow")?
        / 100
        * 100;
    let occupied = events.iter().filter_map(event_time).max().unwrap_or(0);
    let base = occupied
        .checked_add(guard)
        .context("Repeat clock overflow")?;
    let exit_base = base.checked_add(guard).context("Repeat clock overflow")?;
    ensure!(
        exit_base <= i64::MAX / 8,
        "Repeat exceeds replay clock range"
    );
    let mut events: Vec<_> = events.into_iter().map(Some).collect();
    let mut take = |index: usize| {
        events
            .get_mut(index)
            .and_then(Option::take)
            .context("Missing repeat event")
    };
    let mut compiled = Vec::new();
    let mut compiled_slots = Vec::new();
    let mut mapping = vec![0; slots.len()];
    for (index, slot) in slots.iter().enumerate().take(begin) {
        *mapping.get_mut(index).context("Missing slot mapping")? = compiled_slots.len();
        compiled_slots.push(slot.clone());
        compiled.push(take(index)?);
    }
    let mut opening = take(begin)?;
    jump(&mut opening, "repeat-0")?;
    *mapping.get_mut(begin).context("Missing opening mapping")? = compiled_slots.len();
    let mut opening_slot = slots.get(begin).context("Missing opening slot")?.clone();
    put(
        contexts.first().context("Missing opening context")?,
        &mut opening_slot,
        "repeatContext",
    )?;
    compiled_slots.push(opening_slot);
    compiled.push(opening);
    let mut clock_at = base;
    for position in 1..=width {
        let old_slots: Vec<_> = if position == width {
            (1..rounds).map(|round| begin + round * width).collect()
        } else {
            (0..rounds)
                .map(|round| begin + round * width + position)
                .collect()
        };
        let mut slot = slots
            .get(*old_slots.first().context("Missing repeat body")?)
            .context("Missing body slot")?
            .clone();
        let mut sample_list = Vec::new();
        let mut clocks = Vec::new();
        for &old in &old_slots {
            *mapping.get_mut(old).context("Missing body mapping")? = compiled_slots.len();
            let previous = old - 1;
            for sample in slots
                .get(old)
                .and_then(|slot| slot["samples"].as_array())
                .context("Missing body samples")?
            {
                let file = sample["file"].as_str().context("Missing body file")?;
                let prior = slots
                    .get(previous)
                    .and_then(|slot| slot["samples"].as_array())
                    .context("Missing previous samples")?
                    .iter()
                    .find(|sample| sample["file"] == file)
                    .context("Missing previous observation")?;
                clocks.push(
                    clock_at + sample["timeMs"].as_i64().context("Missing sample time")?
                        - prior["timeMs"].as_i64().context("Missing previous time")?,
                );
                let origin_position = if position == width {
                    old - width
                } else {
                    old - position
                };
                let origin = slots
                    .get(origin_position)
                    .and_then(|slot| slot["samples"].as_array())
                    .context("Missing round origin")?
                    .iter()
                    .find(|sample| sample["file"] == file)
                    .and_then(|sample| sample["timeMs"].as_i64())
                    .context("Missing round origin time")?;
                let mut sample = sample.clone();
                put(origin, &mut sample, "blockEntryMs")?;
                let relative = sample
                    .get("timeMs")
                    .and_then(Value::as_i64)
                    .context("Missing relative sample time")?
                    - origin;
                put(relative, &mut sample, "relativeMs")?;
                sample_list.push(sample);
            }
        }
        put(
            statistics(
                sample_list
                    .iter()
                    .filter_map(|sample| sample["relativeMs"].as_i64())
                    .collect(),
            )?,
            &mut slot,
            "time",
        )?;
        put(sample_list, &mut slot, "samples")?;
        put(
            contexts
                .get(position % width)
                .context("Missing body context")?,
            &mut slot,
            "repeatContext",
        )?;
        let mut entry = take(*old_slots.first().context("Missing body event")?)?;
        let at = place(&mut entry, &mut slot, clocks)?;
        ensure!(at > clock_at, "repeat timing does not advance");
        if position == width {
            jump(&mut entry, "repeat-0")?;
        }
        clock_at = at;
        compiled_slots.push(slot);
        compiled.push(entry);
    }
    let mut exit_entry = take(exit_slot)?;
    let mut exit_evidence = slots
        .get(exit_slot)
        .context("Missing exit evidence")?
        .clone();
    let prior_position = exit_slot - 1;
    let prior_at = if width == 1 {
        base
    } else {
        compiled
            .iter()
            .filter_map(event_time)
            .nth_back(1)
            .context("Missing final body time")?
    };
    let clocks = exit_evidence
        .get("samples")
        .and_then(Value::as_array)
        .context("Missing exit samples")?
        .iter()
        .map(|sample| {
            let prior = slots
                .get(prior_position)
                .and_then(|slot| slot["samples"].as_array())
                .context("Missing exit predecessor")?
                .iter()
                .find(|prior| prior["file"] == sample["file"])
                .context("Missing exit predecessor sample")?;
            Ok(
                prior_at + sample["timeMs"].as_i64().context("Missing exit time")?
                    - prior["timeMs"]
                        .as_i64()
                        .context("Missing exit predecessor time")?,
            )
        })
        .collect::<Result<_>>()?;
    let exit_at = place(&mut exit_entry, &mut exit_evidence, clocks)?;
    ensure!(exit_at > prior_at, "exit timing does not advance");
    jump(&mut exit_entry, "repeat-exit-0")?;
    put(exit_context, &mut exit_evidence, "repeatContext")?;
    *mapping.get_mut(exit_slot).context("Missing exit mapping")? = compiled_slots.len();
    compiled_slots.push(exit_evidence);
    compiled.push(exit_entry);
    let old_exit_at = slots
        .get(exit_slot)
        .and_then(|slot| slot["atMs"].as_f64())
        .context("Missing finite exit time")?
        .round() as i64;
    for (index, slot) in slots.iter().enumerate().skip(exit_slot + 1) {
        let mut entry = take(index)?;
        let mut slot = slot.clone();
        let at = event_time(&entry).context("Missing suffix time")? - old_exit_at + exit_base;
        if let Entry::Event { at: time, .. } = &mut entry {
            *time = at as f64 / 1000.0;
        }
        put(at, &mut slot, "atMs")?;
        *mapping.get_mut(index).context("Missing suffix mapping")? = compiled_slots.len();
        compiled_slots.push(slot);
        compiled.push(entry);
    }
    // Sort evidence beside its event, e.g. an exit at +8s precedes a continuation at +10s.
    let mut ordered: Vec<_> = compiled
        .into_iter()
        .zip(compiled_slots)
        .enumerate()
        .collect();
    ordered.sort_by_key(|(_, (entry, _))| event_time(entry));
    let mut reordered = BTreeMap::new();
    let mut rows = Vec::new();
    let mut compiled_slots = Vec::new();
    for (index, (old, (entry, slot))) in ordered.into_iter().enumerate() {
        reordered.insert(old, index);
        rows.push(entry);
        compiled_slots.push(slot);
    }
    for slot in &mut mapping {
        *slot = *reordered.get(slot).context("Missing sorted repeat slot")?;
    }
    // Reindex generated diagnostics too, e.g. a disabled suffix now refers to its compressed slot.
    for (index, entry) in rows.iter_mut().enumerate() {
        if let Entry::Event {
            sync: Some(Sync::Network(sync)),
            note,
            ..
        } = entry
            && !sync.enabled
            && let Some(conflict) = report["syncConflicts"].as_array().and_then(|conflicts| {
                conflicts.iter().find(|conflict| {
                    conflict["slot"]
                        .as_u64()
                        .and_then(|old| mapping.get(old as usize))
                        == Some(&index)
                })
            })
        {
            *note = Some(format!(
                "Sync disabled: {}; see slot {index} in report",
                conflict["reason"]
                    .as_str()
                    .context("Missing conflict reason")?
            ));
        }
    }
    rows.push(Entry::Label {
        at: base as f64 / 1000.0,
        name: "repeat-0".into(),
    });
    rows.push(Entry::Label {
        at: exit_base as f64 / 1000.0,
        name: "repeat-exit-0".into(),
    });
    rows.sort_by_key(event_time);
    rows.insert(0, Entry::Note { text: "Draft with an observed conditional repeat; sync-only continuation and exit. Supported occurrence counts and context are in the report; runtime unverified.".into() });
    rows.extend(
        other
            .into_iter()
            .filter(|entry| matches!(entry, Entry::AbilityCatalog { .. })),
    );
    let compiled_yaml = draft::serialize_draft(rows, timeline.reset_on)?;
    let mut result = report.clone();
    for (index, slot) in compiled_slots.iter_mut().enumerate() {
        slot["id"] = index.into();
        let at = slot["atMs"]
            .as_f64()
            .context("Missing compiled slot time")?
            .round() as i64;
        slot["block"] = if at < base {
            0
        } else if at < exit_base {
            1
        } else {
            2
        }
        .into();
    }
    put(compiled_slots, &mut result, "slots")?;
    put(
        json!({"accepted": true, "lookaheadMs": horizon, "finiteYaml": yaml,
        "finiteEvidence": {"group": report["group"], "slots": report["slots"], "extensions": report["extensions"]},
        "slotMapping": mapping, "entryMs": base, "exitMs": exit_base,
        "observedMaxRounds": rounds, "forcejumpGenerated": false}),
        &mut result,
        "repeats",
    )?;
    put(report["blocks"].clone(), &mut result, "finiteBlocks")?;
    put(
        json!([
            {"id": 0, "entry": "fightStart", "draftEntryMs": 0, "time": statistics(vec![0; inputs.len()])?},
            {"id": 1, "entry": "observedRepeatSync", "label": "repeat-0", "draftEntryMs": base,
                "time": candidate["period"]},
            {"id": 2, "entry": "observedRepeatExit", "label": "repeat-exit-0", "draftEntryMs": exit_base,
                "time": candidate["exitOffset"]}
        ]),
        &mut result,
        "blocks",
    )?;
    let mut conflicts = report["syncConflicts"]
        .as_array()
        .context("Missing finite conflicts")?
        .clone();
    for conflict in &mut conflicts {
        let old = conflict["slot"].as_u64().context("Missing conflict slot")? as usize;
        put(
            *mapping.get(old).context("Missing conflict mapping")?,
            conflict,
            "slot",
        )?;
    }
    put(conflicts, &mut result, "syncConflicts")?;
    let group: super::GroupKey = serde_json::from_value(report["group"].clone())?;
    let peers = inputs
        .iter()
        .map(|input| (input.pull.file.as_str(), &input.pull))
        .collect();
    let mut checks = Vec::new();
    for input in inputs {
        let evidence = replay::evidence(
            &compiled_yaml,
            &result,
            &group,
            &input.pull,
            &input.log,
            &peers,
        )?;
        let check = crate::timeline::replay::run(
            &compiled_yaml,
            &replay::signals(&input.log)?,
            input.pull.end_ms,
            &evidence,
        )?;
        checks.push(
            json!({"file": input.pull.file, "passed": check.passed, "summary": check.summary,
            "jumps": check.jumps, "previews": check.previews}),
        );
    }
    candidate["checks"] = serde_json::to_value(&checks)?;
    ensure!(
        checks.iter().all(|check| check["passed"] == true),
        "candidate failed independent raw-signal replay"
    );
    put(
        true,
        result.get_mut("validation").context("Missing validation")?,
        "replay",
    )?;
    put(
        json!([
            "Conditional repeat is validated only for observed finite occurrence counts and raw-signal contexts.",
            "No unbounded repeat inference, forcejump, nested loops or cross-encounter connections.",
            "Missing exits retain isolated virtual blocks over observed duration and configured lookahead.",
            "cactbot parser/runtime, display, priority and reset compatibility remain unverified."
        ]),
        &mut result,
        "limitations",
    )?;
    Ok((compiled_yaml, result, serde_json::to_value(checks)?))
}

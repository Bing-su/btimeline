//! Fold a repeated boss suffix without inventing an exit, e.g. a dungeon boss loops until defeated.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::{Pull, alignment, draft, multi, replay, sections};
use super::{Input, Signal, SignalKey, event_time, put, sample_index};
use crate::fflogs::model::CollectedLog;
use crate::timeline::{Destination, Entry, Jump, JumpWhen, Sync, Timeline};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Step {
    key: SignalKey,
    ability_ids: Vec<i64>,
    paired_start: bool,
    targetable: Option<bool>,
}

// Ignore target fan-out and transient helper instance numbers; boss attempts are bounded separately.
fn key(signal: &Signal) -> SignalKey {
    let mut key = alignment::match_key(signal);
    key.ability_id = 0;
    key
}

impl Step {
    fn matches(&self, signal: &Signal, pull: &Pull, log: &CollectedLog) -> bool {
        self.key == key(signal)
            && self.ability_ids.contains(&signal.key.ability_id)
            && self.paired_start == paired(signal, pull)
            && self.targetable == targetable(signal, log)
    }

    fn overlaps(&self, other: &Self) -> bool {
        self.key == other.key
            && self.paired_start == other.paired_start
            && self.targetable == other.targetable
            && self
                .ability_ids
                .iter()
                .any(|id| other.ability_ids.contains(id))
    }
}

// Compare the caster's state only; unrelated actors and earlier despawned helper instances do not define this step.
fn targetable(signal: &Signal, log: &CollectedLog) -> Option<bool> {
    let actor_ids: BTreeSet<_> = signal
        .event_indices
        .iter()
        .filter_map(|&index| log.events.get(index)?.source_id)
        .collect();
    let index = signal.event_indices.first().copied()?;
    let cast = log.events.get(index)?;
    log.events
        .iter()
        .enumerate()
        .filter(|(i, event)| {
            event.kind == "targetabilityupdate"
                && (event.timestamp, *i) < (cast.timestamp, index)
                && event
                    .target_id
                    .or(event.source_id)
                    .is_some_and(|id| actor_ids.contains(&id))
                && (signal.instance_ids.is_empty()
                    || event
                        .source_instance
                        .is_none_or(|instance| signal.instance_ids.contains(&instance)))
        })
        .max_by_key(|(i, event)| (event.timestamp, *i))
        .and_then(|(_, event)| event.extra.get("targetable")?.as_bool())
}

fn paired(signal: &Signal, pull: &Pull) -> bool {
    signal.event_indices.iter().all(|index| {
        pull.occurrences
            .iter()
            .any(|row| row.event_index == *index && row.start_event_index.is_some())
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Loop {
    section: usize,
    start: usize,
    width: usize,
    end: usize,
    entry_ordinal: usize,
    steps: Vec<Step>,
    offsets_ms: Vec<i64>,
    opening_slot: usize,
    body_slots: Vec<usize>,
}

fn events(timeline: &Timeline) -> Vec<usize> {
    timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Event { .. }) && !entry.is_combat_start())
        .map(|(index, _)| index)
        .collect()
}

fn step(slot: &Value, inputs: &[Input]) -> Result<Step> {
    let (input, signal) = inputs
        .iter()
        .find_map(|input| {
            let index = sample_index(slot, &input.pull.file)?;
            Some((input, input.signals.get(&index)?))
        })
        .context("Missing loop source sample")?;
    Ok(Step {
        key: key(signal),
        ability_ids: serde_json::from_value(slot["abilityIds"].clone())?,
        paired_start: paired(signal, &input.pull),
        targetable: targetable(signal, &input.log),
    })
}

pub(in super::super) fn compress(
    yaml: String,
    report: Value,
    inputs: &[Input],
    lookahead: f64,
) -> Result<(String, Value)> {
    // Preserve the existing conditional compiler, e.g. its observed exit needs different evidence.
    if report.pointer("/repeats/accepted") == Some(&json!(true)) {
        return Ok((yaml, report));
    }
    let timeline: Timeline = serde_saphyr::from_str(&yaml)?;
    let entries = events(&timeline);
    let slots = report
        .get("slots")
        .and_then(Value::as_array)
        .context("Missing loop slots")?;
    let sources: Vec<_> = inputs.iter().map(|input| &input.report).collect();
    let ids = slots
        .iter()
        .map(|slot| {
            if report.get("mode").and_then(Value::as_str) == Some("raid") {
                Ok(0)
            } else {
                sections::slot_section(slot, &sources, false)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    let steps = slots
        .iter()
        .map(|slot| step(slot, inputs))
        .collect::<Result<Vec<_>>>()?;
    let times = entries
        .iter()
        .map(|&entry| event_time(timeline.entries.get(entry)?))
        .collect::<Option<Vec<_>>>()
        .context("Missing loop times")?;
    let mut accepted = Vec::new();
    let mut candidates = Vec::new();
    let mut result = None;
    for section in ids.iter().copied().collect::<BTreeSet<_>>() {
        let positions: Vec<_> = ids
            .iter()
            .enumerate()
            .filter(|(_, id)| **id == section)
            .map(|(i, _)| i)
            .collect();
        let Some(&begin) = positions.first() else {
            continue;
        };
        let end = positions.last().copied().context("Missing loop end")? + 1;
        // Keep unresolved control flow finite; labels inside a body cannot be removed independently.
        if timeline.entries.iter().any(|entry| {
            matches!(entry, Entry::Label { .. })
                && event_time(entry).is_some_and(|at| {
                    times.get(begin).is_some_and(|&lo| lo <= at)
                        && times.get(end - 1).is_some_and(|&hi| at <= hi)
                })
        }) {
            continue;
        }
        'search: for start in begin..end {
            if (start..end).any(|i| !matches!(entries.get(i).and_then(|&entry| timeline.entries.get(entry)),
                Some(Entry::Event { sync: Some(Sync::Network(sync)), jump: None, .. }) if sync.enabled)) { continue; }
            // A cast and its effect can recur before a different body, e.g. Chort's Gloom.
            for width in 3..end.saturating_sub(start + 2) {
                let Some(&closing) = times.get(start + width) else {
                    continue;
                };
                let origin = *times.get(start).context("Missing loop origin")?;
                if closing - origin < 10_000 {
                    continue;
                }
                let body = steps
                    .get(start..start + width)
                    .context("Missing loop body")?;
                let opening = body.first().context("Missing loop opening")?;
                if body.iter().skip(1).any(|step| opening.overlaps(step)) {
                    continue;
                }
                if !(start + width..end).all(|i| {
                    steps
                        .get(i)
                        .zip(body.get((i - start) % width))
                        .is_some_and(|(a, b)| a.overlaps(b))
                }) {
                    continue;
                }
                let (input, signal) = inputs
                    .iter()
                    .find_map(|input| {
                        let index = sample_index(slots.get(start)?, &input.pull.file)?;
                        Some((input, input.signals.get(&index)?))
                    })
                    .context("Missing loop opening sample")?;
                let lo = if report.get("mode").and_then(Value::as_str) == Some("raid") {
                    0
                } else {
                    input
                        .pull
                        .boss_spans
                        .get(section)
                        .map(|span| span.0)
                        .context("Missing loop boss span")?
                };
                let entry_ordinal = input
                    .signals
                    .values()
                    .filter(|other| {
                        other.time_ms >= lo
                            && other.time_ms < signal.time_ms
                            && opening.matches(other, &input.pull, &input.log)
                    })
                    .count();
                let candidate = Loop {
                    section,
                    start,
                    width,
                    end,
                    entry_ordinal,
                    steps: body.to_vec(),
                    offsets_ms: times
                        .get(start..start + width)
                        .context("Missing loop offsets")?
                        .iter()
                        .map(|at| at - origin)
                        .collect(),
                    opening_slot: 0,
                    body_slots: Vec::new(),
                };
                let mut loops = accepted.clone();
                loops.push(candidate);
                let attempt = compile(&yaml, &report, &loops, inputs, lookahead);
                let mut diagnostic = json!({"kind":"unbounded", "section":section,"referenceFile":input.pull.file,
                    "start":start,"width":width,"rounds":(end-start).div_ceil(width),
                    "period":{"minMs":closing-origin,"maxMs":closing-origin},"accepted":false});
                match attempt {
                    Ok((compiled, mut evidence)) => {
                        put(true, &mut diagnostic, "accepted")?;
                        put(
                            "matching next-cycle prefix and independent raw replay passed",
                            &mut diagnostic,
                            "reason",
                        )?;
                        candidates.push(diagnostic);
                        accepted = loops;
                        put(
                            &candidates,
                            evidence.get_mut("repeats").context("Missing loops")?,
                            "candidates",
                        )?;
                        result = Some((compiled, evidence));
                        break 'search;
                    }
                    Err(error) => {
                        put(format!("{error:#}"), &mut diagnostic, "reason")?;
                        candidates.push(diagnostic);
                    }
                }
            }
        }
    }
    if let Some((compiled, mut evidence)) = result {
        put(
            candidates,
            evidence.get_mut("repeats").context("Missing loops")?,
            "candidates",
        )?;
        Ok((compiled, evidence))
    } else {
        let mut report = report;
        if !candidates.is_empty() {
            put(candidates, &mut report, "unboundedRepeatCandidates")?;
        }
        Ok((yaml, report))
    }
}

fn compile(
    yaml: &str,
    report: &Value,
    loops: &[Loop],
    inputs: &[Input],
    lookahead: f64,
) -> Result<(String, Value)> {
    let mut timeline: Timeline = serde_saphyr::from_str(yaml)?;
    let old_entries = events(&timeline);
    let slots = report["slots"].as_array().context("Missing finite slots")?;
    let mut mapping: Vec<_> = (0..slots.len()).collect();
    let mut remove = BTreeSet::new();
    for cycle in loops {
        for old in cycle.start + cycle.width + 1..cycle.end {
            *mapping.get_mut(old).context("Missing loop mapping")? =
                cycle.start + 1 + (old - cycle.start - 1) % cycle.width;
            remove.insert(*old_entries.get(old).context("Missing repeated row")?);
        }
        let target = event_time(
            timeline
                .entries
                .get(*old_entries.get(cycle.start).context("Missing opening")?)
                .context("Missing opening time")?,
        )
        .context("Missing opening time")?;
        let label = format!("boss-{}-loop", cycle.section);
        let closing = *old_entries
            .get(cycle.start + cycle.width)
            .context("Missing closing")?;
        let prior_at = old_entries
            .get(cycle.start + cycle.width - 1)
            .and_then(|&entry| timeline.entries.get(entry))
            .and_then(event_time)
            .context("Missing loop predecessor")?;
        let latest = (cycle.start + cycle.width..cycle.end)
            .step_by(cycle.width)
            .flat_map(|old| {
                slots
                    .get(old)
                    .and_then(|slot| slot.get("samples"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(move |sample| {
                        let previous = slots
                            .get(old - 1)?
                            .get("samples")?
                            .as_array()?
                            .iter()
                            .find(|prior| prior.get("file") == sample.get("file"))?;
                        Some(
                            prior_at + sample.get("timeMs")?.as_i64()?
                                - previous.get("timeMs")?.as_i64()?,
                        )
                    })
            })
            .max()
            .context("Missing loop continuation timing")?;
        // Sync before the timer fallback on observed data, e.g. a +30ms cast must not wrap twice.
        if let Some(Entry::Event {
            at,
            sync: Some(Sync::Network(sync)),
            jump,
            ..
        }) = timeline.entries.get_mut(closing)
        {
            let old_at = *at;
            *at = ((latest.max((*at * 1000.0).round() as i64) + 99) / 100 + 1) as f64 / 10.0;
            let mut window = sync.window.unwrap_or([2.5, 2.5]);
            window[0] = ((window[0] + *at - old_at) * 10.0).round() / 10.0;
            sync.window = Some(window);
            *jump = Some(Jump {
                to: Destination::Label(label.clone()),
                when: JumpWhen::Always,
            });
        }
        timeline.entries.push(Entry::Label {
            at: target as f64 / 1000.0,
            name: label,
        });
    }
    let mut kept = Vec::new();
    for (index, entry) in timeline.entries.into_iter().enumerate() {
        if !remove.contains(&index) {
            kept.push(entry);
        }
    }
    kept.sort_by_key(|entry| {
        (
            matches!(entry, Entry::AbilityCatalog { .. }),
            event_time(entry),
        )
    });
    timeline.entries = kept;
    let new_entries = events(&timeline);
    let kept_slots: Vec<_> = (0..slots.len())
        .filter(|&i| !remove.contains(old_entries.get(i).unwrap_or(&usize::MAX)))
        .collect();
    let positions: BTreeMap<_, _> = kept_slots
        .iter()
        .enumerate()
        .map(|(new, &old)| (old, new))
        .collect();
    for slot in &mut mapping {
        *slot = *positions.get(slot).context("Missing folded slot")?;
    }
    let mut compiled_slots = Vec::new();
    for (new, &old) in kept_slots.iter().enumerate() {
        let mut slot = slots.get(old).context("Missing retained slot")?.clone();
        let ability_ids: BTreeSet<i64> = mapping
            .iter()
            .enumerate()
            .filter(|(_, target)| **target == new)
            .flat_map(|(old, _)| {
                slots
                    .get(old)
                    .and_then(|slot| slot.get("abilityIds"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
            })
            .collect();
        put(&ability_ids, &mut slot, "abilityIds")?;
        if let Some(Entry::Event {
            sync: Some(Sync::Network(sync)),
            ..
        }) = new_entries
            .get(new)
            .and_then(|&entry| timeline.entries.get_mut(entry))
        {
            sync.fields.insert(
                "id".into(),
                multi::field_pattern(ability_ids.iter().map(|id| format!("{id:X}")).collect())?,
            );
        }
        let mut samples = Vec::new();
        for (old, _) in mapping
            .iter()
            .enumerate()
            .filter(|(_, target)| **target == new)
        {
            for sample in slots
                .get(old)
                .and_then(|slot| slot.get("samples"))
                .and_then(Value::as_array)
                .context("Missing folded samples")?
            {
                let mut sample = sample.clone();
                if let Some(cycle) = loops
                    .iter()
                    .find(|cycle| cycle.start < old && old < cycle.end)
                {
                    let origin = old - ((old - cycle.start - 1) % cycle.width + 1);
                    let origin = slots
                        .get(origin)
                        .and_then(|slot| slot.get("samples"))
                        .and_then(Value::as_array)
                        .and_then(|samples| {
                            samples
                                .iter()
                                .find(|origin| origin.get("file") == sample.get("file"))
                        })
                        .and_then(|sample| sample.get("timeMs"))
                        .and_then(Value::as_i64)
                        .context("Missing source round origin")?;
                    let relative = sample
                        .get("timeMs")
                        .and_then(Value::as_i64)
                        .context("Missing source round time")?
                        - origin;
                    put(origin, &mut sample, "blockEntryMs")?;
                    put(relative, &mut sample, "relativeMs")?;
                }
                samples.push(sample);
            }
        }
        for (field, source) in [("time", "relativeMs"), ("absoluteTime", "timeMs")] {
            let observations = samples
                .iter()
                .filter_map(|sample| sample.get(source).and_then(Value::as_i64))
                .collect();
            put(multi::statistics(observations)?, &mut slot, field)?;
        }
        put(samples, &mut slot, "samples")?;
        put(new, &mut slot, "id")?;
        put(
            event_time(
                timeline
                    .entries
                    .get(*new_entries.get(new).context("Missing compiled event")?)
                    .context("Missing compiled event")?,
            )
            .context("Missing compiled time")?,
            &mut slot,
            "atMs",
        )?;
        if let Some(Entry::Event {
            sync: Some(Sync::Network(sync)),
            ..
        }) = new_entries
            .get(new)
            .and_then(|&entry| timeline.entries.get(entry))
        {
            put(
                sync.window
                    .unwrap_or([2.5, 2.5])
                    .map(|time| (time * 1000.0).round() as i64),
                &mut slot,
                "windowMs",
            )?;
        }
        compiled_slots.push(slot);
        if let Some(conflict) = report
            .get("syncConflicts")
            .and_then(Value::as_array)
            .and_then(|conflicts| {
                conflicts.iter().find(|conflict| {
                    conflict.get("slot").and_then(Value::as_u64) == Some(old as u64)
                })
            })
            && let Some(Entry::Event { note, .. }) = new_entries
                .get(new)
                .and_then(|&entry| timeline.entries.get_mut(entry))
        {
            *note = Some(format!(
                "Sync disabled: {}; see slot {new} in report",
                conflict
                    .get("reason")
                    .and_then(Value::as_str)
                    .context("Missing conflict reason")?
            ));
        }
    }
    let mut compiled_loops = loops.to_vec();
    for cycle in &mut compiled_loops {
        cycle.opening_slot = *mapping
            .get(cycle.start)
            .context("Missing loop opening mapping")?;
        cycle.body_slots = (cycle.start + 1..=cycle.start + cycle.width)
            .map(|old| {
                mapping
                    .get(old)
                    .copied()
                    .context("Missing loop body mapping")
            })
            .collect::<Result<_>>()?;
        for (offset, step) in cycle.steps.iter_mut().enumerate() {
            step.ability_ids = (cycle.start + offset..cycle.end)
                .step_by(cycle.width)
                .flat_map(|old| {
                    slots
                        .get(old)
                        .and_then(|slot| slot.get("abilityIds"))
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_i64)
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        for (offset, step) in cycle.steps.iter().enumerate() {
            let targets = if offset == 0 {
                vec![
                    cycle.opening_slot,
                    *cycle
                        .body_slots
                        .last()
                        .context("Missing continuation slot")?,
                ]
            } else {
                vec![
                    *cycle
                        .body_slots
                        .get(offset - 1)
                        .context("Missing cyclic slot")?,
                ]
            };
            for slot in targets {
                put(
                    &step.ability_ids,
                    compiled_slots
                        .get_mut(slot)
                        .context("Missing cyclic slot evidence")?,
                    "abilityIds",
                )?;
                if let Some(Entry::Event {
                    sync: Some(Sync::Network(sync)),
                    ..
                }) = new_entries
                    .get(slot)
                    .and_then(|&entry| timeline.entries.get_mut(entry))
                {
                    sync.fields.insert(
                        "id".into(),
                        multi::field_pattern(
                            step.ability_ids
                                .iter()
                                .map(|id| format!("{id:X}"))
                                .collect(),
                        )?,
                    );
                }
            }
        }
    }
    for entry in &mut timeline.entries {
        if let Entry::Note { text } = entry
            && text.starts_with("Draft with")
        {
            *text = "Observed boss suffixes repeat until combat ends; forcejump supplies next-round lookahead. Finite source evidence and rejected candidates remain in the report.".into();
        }
    }
    let compiled = format!(
        "{}{}",
        draft::SCHEMA_HEADER,
        serde_saphyr::to_string(&timeline)?
    );
    crate::timeline::convert(&compiled)?;
    let mut result = report.clone();
    let mut finite = report.clone();
    finite
        .as_object_mut()
        .context("Missing finite evidence")?
        .remove("repeats");
    put(compiled_slots, &mut result, "slots")?;
    if let Some(conflicts) = result
        .get_mut("syncConflicts")
        .and_then(Value::as_array_mut)
    {
        for conflict in conflicts {
            let old = conflict
                .get("slot")
                .and_then(Value::as_u64)
                .and_then(|slot| usize::try_from(slot).ok())
                .context("Missing conflict slot")?;
            put(
                mapping.get(old).context("Missing conflict mapping")?,
                conflict,
                "slot",
            )?;
        }
    }
    // Keep diagnostic selectors aligned too, e.g. a branch in boss 3 follows a compressed boss 1.
    fn remap_paths(value: &mut Value, mapping: &[usize]) -> Result<()> {
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    if name == "selectorSlot" {
                        let old = serde_json::from_value::<usize>(value.clone())?;
                        *value = json!(mapping.get(old).context("Missing selector mapping")?);
                    } else if name == "slots"
                        && let Some(slots) = value.as_array_mut()
                    {
                        for slot in slots {
                            let old = serde_json::from_value::<usize>(slot.clone())?;
                            *slot = json!(mapping.get(old).context("Missing path mapping")?);
                        }
                    } else {
                        remap_paths(value, mapping)?;
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    remap_paths(value, mapping)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    if let Some(extensions) = result.get_mut("extensions") {
        for name in ["branches", "phases"] {
            if let Some(value) = extensions.get_mut(name) {
                remap_paths(value, &mapping)?;
            }
        }
    }
    put(
        json!({"accepted":true,"kind":"unbounded","lookaheadMs":(lookahead * 1000.0).ceil() as i64,
        "finiteYaml":yaml,"finiteEvidence":finite,"slotMapping":mapping,"loops":compiled_loops,
        "forcejumpGenerated":true,"candidates":[]}),
        &mut result,
        "repeats",
    )?;
    put(
        json!([
            "Unbounded repeats extrapolate observed boss suffixes; unseen later phases remain unknown.",
            "Role, cast pairing, ID alternatives, complete observed sequences and raw sync windows must agree.",
            "Branching or unresolved repeat bodies retain finite rows.",
            "Live ACT delivery, game behavior and browser layout require separate validation."
        ]),
        &mut result,
        "limitations",
    )?;
    // Reindex section starts after deletions, e.g. boss 3 still starts from zero after boss 1 loops.
    if let Some(sections) = result.get_mut("bossSections").and_then(Value::as_array_mut) {
        for section in sections {
            if let Some(indices) = section
                .get_mut("entryIndices")
                .and_then(Value::as_array_mut)
            {
                for index in indices {
                    let old = old_entries
                        .iter()
                        .position(|&entry| Some(entry as u64) == index.as_u64())
                        .context("Missing old section entry")?;
                    *index = json!(
                        new_entries
                            .get(*mapping.get(old).context("Missing section mapping")?)
                            .context("Missing new section entry")?
                    );
                }
            }
        }
    }
    let group = serde_json::from_value(report["group"].clone())?;
    let peers = inputs
        .iter()
        .map(|input| (input.pull.file.as_str(), &input.pull))
        .collect();
    let mut checks = Vec::new();
    let loops: Vec<Loop> = serde_json::from_value(
        result
            .pointer("/repeats/loops")
            .context("Missing loop support")?
            .clone(),
    )?;
    let mut support: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    for input in inputs {
        let evidence =
            replay::evidence(&compiled, &result, &group, &input.pull, &input.log, &peers)?;
        for cycle in &loops {
            let continuation = cycle
                .body_slots
                .last()
                .and_then(|slot| new_entries.get(*slot));
            let continued = continuation
                .and_then(|entry| evidence.expected.get(entry))
                .is_some_and(|indices| !indices.is_empty());
            let repeated_steps = cycle
                .body_slots
                .iter()
                .take(cycle.width - 1)
                .filter(|&&slot| {
                    new_entries
                        .get(slot)
                        .and_then(|entry| evidence.expected.get(entry))
                        .is_some_and(|indices| indices.len() >= 2)
                })
                .count();
            // Look beyond the opening's effect, e.g. Gloom alone does not prove another Emission.
            if continued && repeated_steps >= 2 {
                support
                    .entry(cycle.section)
                    .or_default()
                    .insert(&input.pull.report);
            }
        }
        let check = crate::timeline::replay::run(
            &compiled,
            &replay::signals(&input.log)?,
            input.pull.end_ms,
            &evidence,
        )?;
        ensure!(
            check.passed,
            "unbounded repeat failed raw replay: {}: {}",
            input.pull.file,
            serde_json::to_string(&check.summary)?
        );
        checks.push(json!({"file":input.pull.file,"passed":check.passed,"summary":check.summary,"jumps":check.jumps,"previews":check.previews}));
    }
    ensure!(
        loops.iter().all(|cycle| support
            .get(&cycle.section)
            .is_some_and(|reports| reports.len() >= 2)),
        "unbounded repeat requires a matching next-cycle prefix in at least two reports"
    );
    put(
        support,
        result
            .get_mut("repeats")
            .context("Missing loop support report")?,
        "supportingReports",
    )?;
    put(
        checks,
        result.get_mut("repeats").context("Missing repeat report")?,
        "checks",
    )?;
    Ok((compiled, result))
}

pub(super) fn extend_evidence(
    timeline: &Timeline,
    report: &Value,
    pull: &Pull,
    log: &CollectedLog,
    result: &mut crate::timeline::replay::Evidence,
) -> Result<()> {
    let loops: Vec<Loop> = serde_json::from_value(
        report
            .pointer("/repeats/loops")
            .context("Missing loop evidence")?
            .clone(),
    )?;
    let entries = events(timeline);
    let mut raw = pull.clone();
    if let Some(occurrences) = raw.raw_occurrences.take() {
        raw.occurrences = occurrences;
    }
    let signals = alignment::signals(&raw);
    let fight = log.report.fights.first().context("Missing loop fight")?;
    let enemies = fight
        .enemy_npcs
        .iter()
        .chain(&fight.enemy_pets)
        .map(|actor| actor.id)
        .collect();
    let (spans, attempts) = draft::boss_spans(log, &enemies);
    let selected = sections::merge_spans(
        spans
            .values()
            .map(|&(lo, hi)| (lo - fight.start_time, hi - fight.start_time))
            .collect(),
    );
    for cycle in loops {
        let rows: Vec<_> = std::iter::once(cycle.opening_slot)
            .chain(cycle.body_slots.iter().copied())
            .map(|slot| {
                entries
                    .get(slot)
                    .copied()
                    .context("Missing loop evidence entry")
            })
            .collect::<Result<_>>()?;
        for (offset, &at) in cycle.offsets_ms.iter().enumerate() {
            if cycle
                .offsets_ms
                .iter()
                .enumerate()
                .any(|(other, &time)| other != offset && at.abs_diff(time) <= 250)
            {
                let slot = if offset == 0 {
                    cycle.opening_slot
                } else {
                    *cycle
                        .body_slots
                        .get(offset - 1)
                        .context("Missing simultaneous loop slot")?
                };
                result.unordered.insert(
                    *entries
                        .get(slot)
                        .context("Missing simultaneous loop entry")?,
                );
            }
        }
        for &entry in &rows {
            result.expected.remove(&entry);
            result.observed_packets.remove(&entry);
            result.inactive.remove(&entry);
            // Preserve finite gaps, e.g. an absent loop opening is not proof that combat ended early.
            if !result.missing.contains(&entry) {
                result.censored.insert(entry);
            }
        }
        let Some(&(lo, hi)) = selected.get(cycle.section) else {
            continue;
        };
        let mut ranges = vec![(lo, hi)];
        for attempt in attempts.iter().filter(|attempt| !attempt.selected) {
            if spans.get(&attempt.actor_id).is_some_and(|&(start, _)| {
                lo <= start - fight.start_time && start - fight.start_time <= hi
            }) {
                ranges.push((attempt.start_ms, attempt.end_ms));
            }
        }
        for (lo, hi) in ranges {
            let mut casts: Vec<_> = signals
                .iter()
                .filter(|signal| {
                    signal.key.kind == "cast" && lo <= signal.time_ms && signal.time_ms <= hi
                })
                .collect();
            casts.sort_by_key(|signal| (signal.time_ms, signal.event_indices.first().copied()));
            let opening = cycle.steps.first().context("Missing loop step")?;
            let Some(start) = casts
                .iter()
                .enumerate()
                .filter(|(_, signal)| opening.matches(signal, &raw, log))
                .nth(cycle.entry_ordinal)
                .map(|(i, _)| i)
            else {
                continue;
            };
            let mut pending = Vec::new();
            for (position, signal) in casts.into_iter().skip(start).enumerate() {
                if pending.is_empty() {
                    let offset = position % cycle.width;
                    let at = *cycle
                        .offsets_ms
                        .get(offset)
                        .context("Missing step offset")?;
                    pending.push(position);
                    // Same-wave effects can reverse their wire order, e.g. Halo and Miasma at one timestamp.
                    while (position + pending.len()) % cycle.width != 0
                        && cycle
                            .offsets_ms
                            .get((position + pending.len()) % cycle.width)
                            .is_some_and(|other| other - at <= 250)
                    {
                        pending.push(position + pending.len());
                    }
                }
                let found = pending.iter().position(|p| {
                    cycle
                        .steps
                        .get(*p % cycle.width)
                        .is_some_and(|step| step.matches(signal, &raw, log))
                });
                let Some(found) = found else {
                    anyhow::bail!(
                        "loop sequence/context differs in {} section {} at event {:?}",
                        pull.file,
                        cycle.section,
                        signal.event_indices.first()
                    );
                };
                let p = pending.remove(found);
                let slot = if p == 0 {
                    cycle.opening_slot
                } else {
                    *cycle
                        .body_slots
                        .get((p - 1) % cycle.width)
                        .context("Missing repeated slot")?
                };
                let entry = *entries.get(slot).context("Missing repeated entry")?;
                result.expected.entry(entry).or_default().insert(
                    *signal
                        .event_indices
                        .first()
                        .context("Empty repeated signal")?,
                );
                result
                    .observed_packets
                    .entry(entry)
                    .or_default()
                    .extend(signal.event_indices.iter().copied());
                result.censored.remove(&entry);
            }
        }
    }
    Ok(())
}

use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::Serialize;
use serde_json::Value;

use super::draft::SCHEMA_HEADER;
use super::phase::put;
use crate::timeline::{Destination, Entry, Sync, Timeline};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Section {
    id: usize,
    original_start_ms: i64,
    original_end_ms: i64,
    offset_ms: i64,
    entry_indices: Vec<usize>,
}

// Merge overlapping boss observations, e.g. two simultaneous bosses share one lifecycle section.
fn spans(input: &Value) -> Result<Vec<(i64, i64)>> {
    let mut spans = input
        .get("bossSegments")
        .and_then(Value::as_array)
        .context("Missing boss segments")?
        .iter()
        .map(|span| {
            Ok((
                span["startMs"].as_i64().context("Missing boss start")?,
                span["endMs"].as_i64().context("Missing boss end")?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    spans.sort();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in spans {
        if let Some(previous) = merged.last_mut()
            && start <= previous.1
        {
            previous.1 = previous.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    Ok(merged)
}

// Use raw timing only to assign a section; virtual P7/P8 jumps remain local to that section.
fn slot_section(slot: &Value, inputs: &[&Value], single: bool) -> Result<usize> {
    let samples = if single {
        vec![(
            inputs.first().copied().context("Missing input")?,
            slot["timeMs"].as_i64().context("Missing single timing")?,
        )]
    } else {
        slot["samples"]
            .as_array()
            .context("Missing section samples")?
            .iter()
            .map(|sample| {
                let input = inputs
                    .iter()
                    .copied()
                    .find(|input| input.pointer("/input/file") == sample.get("file"))
                    .context("Missing section input")?;
                Ok((
                    input,
                    sample["timeMs"].as_i64().context("Missing sample timing")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?
    };
    let sections = samples
        .into_iter()
        .map(|(input, at)| {
            spans(input)?
                .iter()
                .position(|&(start, end)| start <= at && at <= end)
                .context("Cast lies outside observed boss sections")
        })
        .collect::<Result<Vec<_>>>()?;
    let section = *sections.first().context("Missing section evidence")?;
    ensure!(
        sections.iter().all(|&other| other == section),
        "Boss section correspondence differs between pulls"
    );
    Ok(section)
}

// Separate consumer clock ranges, e.g. boss 2 remains reachable from zero after a 7DE reset.
pub(super) fn separate(yaml: String, mut report: Value, lookahead: f64) -> Result<(String, Value)> {
    if report.get("mode").and_then(Value::as_str) == Some("raid") {
        return Ok((yaml, report));
    }
    let inputs: Vec<_> = if report.get("input").is_some() {
        vec![&report]
    } else {
        report
            .get("inputs")
            .and_then(Value::as_array)
            .context("Missing section inputs")?
            .iter()
            .collect()
    };
    let mut timeline: Timeline = serde_saphyr::from_str(&yaml)?;
    let events: Vec<_> = timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Event { .. }) && !entry.is_combat_start())
        .map(|(index, _)| index)
        .collect();
    let slots = report
        .get("slots")
        .and_then(Value::as_array)
        .context("Missing section slots")?;
    ensure!(events.len() == slots.len(), "Section slot count differs");
    let ids = slots
        .iter()
        .map(|slot| slot_section(slot, &inputs, report.get("input").is_some()))
        .collect::<Result<Vec<_>>>()?;
    // Inspect observed bosses, not emitted rows, e.g. consensus may omit every cast of boss 1.
    let multiple = inputs
        .iter()
        .map(|input| spans(input).map(|spans| spans.len() > 1))
        .collect::<Result<Vec<_>>>()?;
    if events.is_empty() || !multiple.into_iter().any(|multiple| multiple) {
        return Ok((yaml, report));
    }
    ensure!(
        ids.windows(2).all(|pair| matches!(pair, [a, b] if a <= b)),
        "Compiled boss sections interleave"
    );
    let time = |entry: &Entry| match entry {
        Entry::Event { at, .. } | Entry::Label { at, .. } => Some((*at * 1000.0).round() as i64),
        _ => None,
    };
    let timed: Vec<_> = timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.is_combat_start())
        .filter_map(|(index, entry)| {
            let at = time(entry)?;
            let position = events
                .iter()
                .position(|&i| {
                    timeline
                        .entries
                        .get(i)
                        .and_then(time)
                        .is_some_and(|time| time >= at)
                })
                .unwrap_or(events.len().saturating_sub(1));
            Some((index, at, *ids.get(position)?))
        })
        .collect();
    let mut sections: Vec<Section> = Vec::new();
    let longest = inputs
        .iter()
        .filter_map(|input| input.get("endMs").and_then(Value::as_i64))
        .max()
        .unwrap_or(0);
    ensure!(
        longest <= i64::MAX / 4,
        "Boss duration exceeds section clock range"
    );
    let mut base = ((longest + 999_999) / 1_000_000 * 1_000_000).max(1_000_000);
    for &id in &ids {
        if sections.last().is_some_and(|section| section.id == id) {
            continue;
        }
        let start = timed
            .iter()
            .filter(|row| row.2 == id)
            .map(|row| row.1)
            .min()
            .context("Missing section start")?;
        let end = timed
            .iter()
            .filter(|row| row.2 == id)
            .map(|row| row.1)
            .max()
            .context("Missing section end")?;
        let entry_index = events.iter().zip(&ids).filter(|(_, section)| **section == id)
            .find(|(index, _)| matches!(timeline.entries.get(**index), Some(Entry::Event { sync: Some(Sync::Network(sync)), .. }) if sync.enabled))
            .map(|(&index, _)| index).context("Boss section has no safe entry sync")?;
        let mut entry_indices = vec![entry_index];
        // Open every first branch choice, e.g. boss 2 may start with either X or Y after a reset.
        if let Some(branches) = report
            .pointer("/extensions/branches")
            .and_then(Value::as_array)
        {
            let position = events
                .iter()
                .position(|&index| index == entry_index)
                .context("Missing entry position")?;
            for branch in branches {
                let paths = branch
                    .get("paths")
                    .and_then(Value::as_array)
                    .context("Missing entry paths")?;
                if paths.iter().any(|path| {
                    path.get("selectorSlot").and_then(Value::as_u64) == Some(position as u64)
                }) {
                    entry_indices = paths
                        .iter()
                        .map(|path| {
                            let slot = usize::try_from(
                                path.get("selectorSlot")
                                    .and_then(Value::as_u64)
                                    .context("Missing selector")?,
                            )?;
                            events.get(slot).copied().context("Missing selector event")
                        })
                        .collect::<Result<_>>()?;
                }
            }
        }
        let after = timed
            .iter()
            .filter(|row| row.2 == id)
            .filter_map(|row| match timeline.entries.get(row.0)? {
                Entry::Event {
                    sync: Some(Sync::Network(sync)),
                    ..
                } => Some((sync.window.unwrap_or([2.5, 2.5])[1] * 1000.0).ceil() as i64),
                _ => None,
            })
            .max()
            .unwrap_or(2500);
        sections.push(Section {
            id,
            original_start_ms: start,
            original_end_ms: end,
            offset_ms: base - start,
            entry_indices,
        });
        // Leave room for observed travel and preview, e.g. boss 2 stays active before an area reset.
        base = base
            .checked_add(end - start)
            .and_then(|value| value.checked_add(after))
            .and_then(|value| value.checked_add(longest))
            .and_then(|value| value.checked_add((lookahead * 1000.0).ceil() as i64 + 999_999))
            .context("Boss section clock overflow")?
            / 1_000_000
            * 1_000_000;
        ensure!(
            base <= i64::MAX / 4,
            "Boss sections exceed replay clock range"
        );
    }
    let shifted = |at: i64| -> i64 {
        let section = sections
            .iter()
            .rev()
            .find(|section| section.original_start_ms <= at)
            .or_else(|| sections.first());
        at + section.map_or(0, |section| section.offset_ms)
    };
    for &(index, at, id) in &timed {
        let section = sections
            .iter()
            .find(|section| section.id == id)
            .context("Missing section")?;
        match timeline
            .entries
            .get_mut(index)
            .context("Missing section row")?
        {
            Entry::Event {
                at: row_at,
                sync,
                jump,
                ..
            } => {
                *row_at = (at + section.offset_ms) as f64 / 1000.0;
                if let Some(jump) = jump
                    && let Destination::Time(to) = &mut jump.to
                    && *to != 0.0
                {
                    *to = shifted((*to * 1000.0).round() as i64) as f64 / 1000.0;
                }
                if section.entry_indices.contains(&index)
                    && let Some(Sync::Network(sync)) = sync
                {
                    // Widen only the entry, e.g. its first cast can select boss 3 from the stopped clock.
                    let mut window = sync.window.unwrap_or([2.5, 2.5]);
                    window[0] = *row_at;
                    sync.window = Some(window);
                }
            }
            Entry::Label { at: row_at, .. } => *row_at = (at + section.offset_ms) as f64 / 1000.0,
            _ => {}
        }
    }
    // Update virtual coordinates without changing source statistics, e.g. sample.timeMs stays FFLogs time.
    fn coordinates(value: &mut Value, shifted: &impl Fn(i64) -> i64) {
        match value {
            Value::Object(fields) => {
                let fight_start = fields.get("entry").and_then(Value::as_str) == Some("fightStart");
                for (key, value) in fields {
                    if matches!(key.as_str(), "draftEntryMs" | "entryMs" | "mergeMs")
                        && let Some(at) = value.as_f64()
                    {
                        // The global start still owns zero, e.g. section offsets do not move InCombat.
                        if !(fight_start && key == "draftEntryMs" && at == 0.0) {
                            *value = shifted(at.round() as i64).into();
                        }
                    } else if !matches!(
                        key.as_str(),
                        "checks" | "finiteEvidence" | "alignmentSlots" | "alignmentBlocks"
                    ) {
                        coordinates(value, shifted);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    coordinates(value, shifted);
                }
            }
            _ => {}
        }
    }
    coordinates(&mut report, &shifted);
    // Record displayed times directly, e.g. a median rounded upward must still use its own section.
    let slots = report
        .get_mut("slots")
        .and_then(Value::as_array_mut)
        .context("Missing section slots")?;
    for (&index, slot) in events.iter().zip(slots) {
        if slot.get("atMs").is_some() {
            put(
                time(
                    timeline
                        .entries
                        .get(index)
                        .context("Missing section event")?,
                )
                .context("Missing event time")?,
                slot,
                "atMs",
            )?;
        }
    }
    for section in &sections {
        for &index in &section.entry_indices {
            let position = events
                .iter()
                .position(|&entry| entry == index)
                .context("Missing entry slot")?;
            let slot = report
                .get_mut("slots")
                .and_then(Value::as_array_mut)
                .and_then(|slots| slots.get_mut(position))
                .context("Missing entry slot")?;
            if let Some(Entry::Event {
                sync: Some(Sync::Network(sync)),
                ..
            }) = timeline.entries.get(index)
            {
                put(
                    sync.window
                        .map(|window| window.map(|seconds| (seconds * 1000.0).round() as i64)),
                    slot,
                    "windowMs",
                )?;
            }
        }
    }
    put(sections, &mut report, "bossSections")?;
    let yaml = format!("{SCHEMA_HEADER}{}", serde_saphyr::to_string(&timeline)?);
    crate::timeline::convert(&yaml).context("Sectioned draft failed validation")?;
    Ok((yaml, report))
}

// Audit broadened entry windows using all raw casts, e.g. an excluded earlier cast must not select a boss.
pub(super) fn check(
    yaml: &str,
    report: &mut Value,
    inputs: &[(&super::Pull, &crate::fflogs::model::CollectedLog)],
) -> Result<()> {
    if report.get("bossSections").is_none() {
        return Ok(());
    }
    let key = serde_json::from_value(
        report
            .get("group")
            .context("Missing section group")?
            .clone(),
    )?;
    let peers = inputs
        .iter()
        .map(|(pull, _)| (pull.file.as_str(), *pull))
        .collect();
    let mut checks = Vec::new();
    let mut clocks: std::collections::BTreeMap<usize, Vec<i64>> = Default::default();
    // Merge observations in input order, e.g. section clock statistics retain the same source ordering.
    let results: Vec<_> = inputs
        .par_iter()
        .map(|(pull, log)| {
            let evidence = super::replay::evidence(yaml, report, &key, pull, log, &peers)?;
            let result = crate::timeline::replay::run(
                yaml,
                &super::replay::signals(log)?,
                pull.end_ms,
                &evidence,
            )?;
            ensure!(
                result.passed,
                "Boss section entry failed raw replay: {}",
                pull.file
            );
            Ok::<_, anyhow::Error>(result)
        })
        .collect();
    for ((pull, _), result) in inputs.iter().zip(results) {
        let result = result?;
        for row in &result.rows {
            clocks.entry(row.entry_index).or_default().extend(
                row.observations
                    .iter()
                    .map(|observation| observation.clock_ms),
            );
        }
        checks.push(super::phase::ReplayCheck {
            file: pull.file.clone(),
            passed: result.passed,
            summary: result.summary,
            jumps: result.jumps,
            previews: result.previews,
        });
    }
    // Refresh corrected-clock statistics too, e.g. a P7 selector now observes the rebased preceding boss.
    let timeline: Timeline = serde_saphyr::from_str(yaml)?;
    let events = timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Event { .. }) && !entry.is_combat_start())
        .map(|(index, _)| index);
    let slots = report
        .get_mut("slots")
        .and_then(Value::as_array_mut)
        .context("Missing section slots")?;
    for (entry, slot) in events.zip(slots) {
        if let Some(times) = clocks.remove(&entry).filter(|times| !times.is_empty()) {
            for field in ["clockTime", "transitionClockRange"] {
                if slot.get(field).is_some() {
                    put(super::multi::statistics(times.clone())?, slot, field)?;
                }
            }
        }
    }
    for name in ["extensions", "repeats"] {
        if let Some(stage) = report.get_mut(name)
            && stage.get("accepted").and_then(Value::as_bool) == Some(true)
        {
            put(&checks, stage, "checks")?;
        }
    }
    put(checks, report, "bossSectionChecks")
}

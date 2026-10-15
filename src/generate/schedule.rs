use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::alignment::Signal;
use super::multi::{Input, field_pattern, statistics};
use super::{GroupKey, Pull, draft};
use crate::fflogs::model::CollectedLog;
use crate::timeline::{Destination, Entry, Jump, JumpWhen, LogType, NetworkSync, Sync};

// Phase-relative observations are independent of packet order, e.g. Past/Future swaps never move round two.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase {
    id: i64,
    source: String,
    ability: i64,
    delay_ms: i64,
    at_ms: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Segment {
    phase: i64,
    start_ms: i64,
    end_ms: i64,
    origin_ms: i64,
    opening: Option<usize>,
    reset: Option<usize>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Sample {
    file: String,
    segment: usize,
    event_indices: Vec<usize>,
    instance_ids: Vec<i64>,
    ability_id: i64,
    time_ms: i64,
    block_entry_ms: i64,
    relative_ms: i64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    id: usize,
    phase: i64,
    source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    source_aliases: Vec<String>,
    name: String,
    at_ms: i64,
    relative_ms: i64,
    ability_ids: Vec<i64>,
    samples: Vec<Sample>,
    evidence: String,
    selector: bool,
    predictive: bool,
    required: bool,
    sync_enabled: bool,
}

impl Slot {
    fn sources(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.source).chain(&self.source_aliases)
    }

    fn accepts_source(&self, source: Option<&String>) -> bool {
        source.is_some_and(|source| self.sources().any(|candidate| candidate == source))
    }
}

fn actor_names(log: &CollectedLog) -> BTreeMap<i64, String> {
    log.report
        .master_data
        .actors
        .iter()
        .map(|a| (a.id, a.name.clone()))
        .collect()
}

// Recover resets from a new instance of the phase's opening cast, e.g. a merged FFLogs P2 contains another P1.
fn segments(pull: &Pull, log: &CollectedLog, phases: &[Phase]) -> Result<Vec<Segment>> {
    let actors = actor_names(log);
    let start = log
        .report
        .fights
        .first()
        .context("Missing schedule fight")?
        .start_time;
    let mut boundaries: BTreeMap<i64, i64> = pull
        .phase_spans
        .iter()
        .map(|&(id, lo, _)| (lo, id))
        .collect();
    for phase in phases {
        let mut instances = BTreeSet::new();
        for row in pull
            .occurrences
            .iter()
            .filter(|row| {
                row.kind == "begincast"
                    && row.role.ends_with("/Boss")
                    && row.ability_id == phase.ability
                    && actors.get(&row.actor_id) == Some(&phase.source)
            })
            .sorted_by_key(|row| row.relative_ms)
        {
            if let Some(instance) = row.instance {
                let repeated = !instances.is_empty() && !instances.contains(&instance);
                instances.insert(instance);
                if repeated {
                    let at = (row.relative_ms - phase.delay_ms).max(0);
                    // Metadata remains authoritative near an opening, e.g. a new P2 actor is not a second transition.
                    if !boundaries
                        .iter()
                        .any(|(&lo, &id)| id == phase.id && lo.abs_diff(at) <= 2500)
                    {
                        boundaries.insert(at, phase.id);
                    }
                }
            }
        }
    }
    let boundaries: Vec<_> = boundaries.into_iter().collect();
    let mut result: Vec<Segment> = Vec::new();
    for (position, &(lo, id)) in boundaries.iter().enumerate() {
        let phase = phases
            .iter()
            .find(|phase| phase.id == id)
            .context("Unknown schedule phase")?;
        let hi = boundaries
            .get(position + 1)
            .map_or(pull.end_ms, |&(next, _)| next - 1);
        let opening = pull
            .occurrences
            .iter()
            .filter(|row| {
                row.kind == "begincast"
                    && row.ability_id == phase.ability
                    && actors.get(&row.actor_id) == Some(&phase.source)
                    && lo <= row.relative_ms
                    && row.relative_ms <= hi
            })
            .min_by_key(|row| (row.relative_ms, row.event_index));
        let reset = if position > 0 && phases.first().is_some_and(|phase| phase.id == id) {
            log.events
                .iter()
                .enumerate()
                .filter(|(_, event)| {
                    event.kind == "zonechange"
                        && result.last().is_some_and(|previous| {
                            previous.origin_ms < event.timestamp - start
                                && event.timestamp - start <= lo
                        })
                })
                .max_by_key(|(_, event)| event.timestamp)
                .map(|(index, _)| index)
        } else {
            None
        };
        result.push(Segment {
            phase: id,
            start_ms: lo,
            end_ms: hi,
            origin_ms: opening.map_or(lo + phase.delay_ms, |row| row.relative_ms),
            opening: opening.map(|row| row.event_index),
            reset,
        });
    }
    Ok(result)
}

fn sample(input: &Input, segment: usize, origin: i64, signal: &Signal) -> Sample {
    Sample {
        file: input.pull.file.clone(),
        segment,
        event_indices: signal.event_indices.clone(),
        instance_ids: signal.instance_ids.clone(),
        ability_id: signal.key.ability_id,
        time_ms: signal.time_ms,
        block_entry_ms: origin,
        relative_ms: signal.time_ms - origin,
    }
}

fn event(slot: &Slot, enabled: bool, window: [f64; 2], jump: bool) -> Result<Entry> {
    Ok(Entry::Event {
        at: slot.at_ms as f64 / 1000.0,
        name: if slot.selector {
            "--sync--".into()
        } else if slot.predictive {
            slot.name.clone()
        } else {
            "--reference--".into()
        },
        duration: None,
        sync: Some(Sync::Network(NetworkSync {
            log: if slot.selector {
                LogType::StartsUsing
            } else {
                LogType::Ability
            },
            fields: BTreeMap::from([
                (
                    "id".into(),
                    field_pattern(
                        slot.ability_ids
                            .iter()
                            .map(|id| format!("{id:X}"))
                            .collect(),
                    )?,
                ),
                (
                    "source".into(),
                    field_pattern(
                        slot.sources()
                            .map(|source| regress::escape(source))
                            .collect(),
                    )?,
                ),
            ]),
            enabled,
            window: Some(window),
        })),
        jump: jump.then(|| Jump {
            to: Destination::Time(slot.at_ms as f64 / 1000.0),
            when: JumpWhen::Sync,
        }),
        note: (!slot.predictive && !slot.selector).then(|| {
            format!(
                "Reference evidence: {}; timing or occurrence is conditional.",
                slot.name
            )
        }),
    })
}

pub(super) fn build(
    key: &GroupKey,
    inputs: &[Input],
    lookahead: f64,
) -> Result<Option<(String, Value)>> {
    if inputs.iter().any(|input| input.pull.phase_spans.is_empty()) {
        return Ok(None);
    }
    let ids: Vec<_> = inputs
        .iter()
        .max_by_key(|input| input.pull.phase_spans.len())
        .into_iter()
        .flat_map(|input| &input.pull.phase_spans)
        .map(|&(id, _, _)| id)
        .unique()
        .collect();
    if ids.len() < 2 {
        return Ok(None);
    }
    // A fixed schedule requires one observed phase order, e.g. an alternate phase graph keeps the path compiler.
    for input in inputs {
        let positions: Option<Vec<_>> = input
            .pull
            .phase_spans
            .iter()
            .map(|&(id, _, _)| ids.iter().position(|phase| *phase == id))
            .collect();
        if positions.is_none_or(|positions| {
            positions
                .windows(2)
                .any(|pair| matches!(pair, [a, b] if a >= b))
        }) {
            return Ok(None);
        }
    }
    let mut phases = Vec::new();
    for id in ids {
        let mut openings: BTreeMap<(String, i64), Vec<i64>> = BTreeMap::new();
        for input in inputs {
            for &(_, lo, hi) in input
                .pull
                .phase_spans
                .iter()
                .filter(|&&(phase, _, _)| phase == id)
            {
                if let Some(row) = input
                    .pull
                    .occurrences
                    .iter()
                    .filter(|row| {
                        row.kind == "begincast"
                            && row.role.ends_with("/Boss")
                            && lo <= row.relative_ms
                            && row.relative_ms <= hi
                    })
                    .min_by_key(|row| (row.relative_ms, row.event_index))
                {
                    openings
                        .entry((
                            input
                                .actors
                                .get(&row.actor_id)
                                .context("Missing opening actor")?
                                .clone(),
                            row.ability_id,
                        ))
                        .or_default()
                        .push(row.relative_ms - lo);
                }
            }
        }
        let Some(((source, ability), times)) = openings
            .into_iter()
            .max_by_key(|(key, times)| (times.len(), std::cmp::Reverse(key.clone())))
        else {
            return Ok(None);
        };
        let delay_ms = statistics(times)?.median_ms.round() as i64;
        let at_ms = if phases.is_empty() {
            (delay_ms + 50) / 100 * 100
        } else {
            phases.len() as i64 * 1000000
        };
        phases.push(Phase {
            id,
            source,
            ability,
            delay_ms,
            at_ms,
        });
    }
    // A shared opening cannot identify two phases, e.g. a repeated raidwide alone needs the legacy path model.
    if phases
        .iter()
        .map(|phase| (&phase.source, phase.ability))
        .unique()
        .count()
        != phases.len()
    {
        return Ok(None);
    }
    let spans = inputs
        .iter()
        .map(|input| segments(&input.pull, &input.log, &phases))
        .collect::<Result<Vec<_>>>()?;
    let horizon = (lookahead * 1000.0).ceil() as i64;
    let mut next = 0i64;
    for (position, phase) in phases.iter_mut().enumerate() {
        if position > 0 {
            phase.at_ms = next;
        }
        let duration = spans
            .iter()
            .flatten()
            .filter(|span| span.phase == phase.id)
            .map(|span| span.end_ms - span.origin_ms)
            .max()
            .unwrap_or(0)
            .max(0);
        // Leave a lookahead gap even for long phases, e.g. a 25-minute phase cannot overlap the next 1000s range.
        let end = phase
            .at_ms
            .checked_add(duration)
            .and_then(|end| end.checked_add(horizon))
            .and_then(|end| end.checked_add(1002599))
            .context("Schedule clock overflow")?;
        next = (end / 1000000)
            .checked_mul(1000000)
            .context("Schedule range overflow")?;
        ensure!(next <= i64::MAX / 4, "Schedule exceeds replay clock range");
    }
    let mut observations = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        for (segment, span) in spans
            .get(i)
            .context("Missing schedule spans")?
            .iter()
            .enumerate()
        {
            for signal in input.signals.values().filter(|signal| {
                signal.key.kind == "cast"
                    && span.start_ms <= signal.time_ms
                    && signal.time_ms <= span.end_ms
            }) {
                let index = signal
                    .event_indices
                    .first()
                    .context("Missing schedule source")?;
                let raw = input
                    .log
                    .events
                    .get(*index)
                    .context("Missing schedule event")?;
                let source = raw
                    .source_id
                    .and_then(|id| input.actors.get(&id))
                    .context("Missing schedule actor")?
                    .clone();
                let name = input
                    .log
                    .report
                    .master_data
                    .abilities
                    .iter()
                    .find(|ability| ability.game_id == signal.key.ability_id)
                    .context("Missing schedule ability")?
                    .name
                    .clone();
                observations.push((
                    span.phase,
                    source,
                    name,
                    sample(input, segment, span.origin_ms, signal),
                ));
            }
        }
    }
    observations.sort_by_key(|(phase, source, name, sample)| {
        (
            *phase,
            source.clone(),
            name.clone(),
            sample.relative_ms,
            sample.file.clone(),
            sample.segment,
        )
    });
    let mut slots: Vec<Slot> = Vec::new();
    for (phase, source, name, sample) in observations {
        let found = slots.iter_mut().rev().find(|slot| {
            slot.phase == phase
                && slot.source == source
                && slot.name == name
                && sample.relative_ms.abs_diff(slot.relative_ms) <= 750
                && slot.samples.iter().all(|other| {
                    other.file != sample.file
                        || other.segment != sample.segment
                        || other.relative_ms.abs_diff(sample.relative_ms) <= 250
                })
        });
        if let Some(slot) = found {
            slot.samples.push(sample);
        } else {
            slots.push(Slot {
                id: 0,
                phase,
                source,
                source_aliases: Vec::new(),
                name,
                at_ms: 0,
                relative_ms: sample.relative_ms,
                ability_ids: Vec::new(),
                samples: vec![sample],
                evidence: "phaseSchedule".into(),
                selector: false,
                predictive: true,
                required: false,
                sync_enabled: false,
            });
        }
    }
    // Reconcile whole rounds before counting support, e.g. a 1s Trine drift or Death Surge's caster alias is not optional.
    for i in 0..slots.len() {
        for j in i + 1..slots.len() {
            let (left, right) = slots.split_at_mut(j);
            let a = left.get_mut(i).context("Missing mechanic round")?;
            let b = right.first_mut().context("Missing mechanic peer")?;
            if a.samples.is_empty()
                || b.samples.is_empty()
                || a.phase != b.phase
                || a.name != b.name
            {
                continue;
            }
            let same_source = a
                .sources()
                .any(|source| b.sources().any(|other| source == other));
            let shared_id = a.samples.iter().any(|sample| {
                b.samples
                    .iter()
                    .any(|other| sample.ability_id == other.ability_id)
            });
            if !same_source && !shared_id {
                continue;
            }
            let mut times = a
                .samples
                .iter()
                .chain(&b.samples)
                .map(|sample| sample.relative_ms);
            let first = times.next().context("Empty mechanic round")?;
            let (lo, hi) = times.fold((first, first), |(lo, hi), at| (lo.min(at), hi.max(at)));
            if hi.abs_diff(lo) > 1500
                || a.samples.iter().any(|sample| {
                    b.samples.iter().any(|other| {
                        sample.file == other.file
                            && sample.segment == other.segment
                            && sample.relative_ms.abs_diff(other.relative_ms) > 250
                    })
                })
            {
                continue;
            }
            let aliases: Vec<_> = a
                .sources()
                .chain(b.sources())
                .filter(|source| *source != &a.source)
                .cloned()
                .sorted()
                .dedup()
                .collect();
            a.source_aliases = aliases;
            a.samples.append(&mut b.samples);
        }
    }
    for slot in slots.iter_mut().filter(|slot| !slot.samples.is_empty()) {
        slot.relative_ms = statistics(
            slot.samples
                .iter()
                .map(|sample| sample.relative_ms)
                .collect(),
        )?
        .median_ms
        .round() as i64;
    }
    // Mutually exclusive effects at one fixed time form a displayed alternative, e.g. Past's End / Future's End.
    for i in 0..slots.len() {
        for j in i + 1..slots.len() {
            let (left, right) = slots.split_at_mut(j);
            let a = left.get_mut(i).context("Missing alternative")?;
            let b = right.first_mut().context("Missing alternative peer")?;
            if !a.samples.is_empty()
                && !b.samples.is_empty()
                && a.phase == b.phase
                && a.source == b.source
                && a.relative_ms.abs_diff(b.relative_ms) <= 250
                && !a.samples.iter().any(|a| {
                    b.samples
                        .iter()
                        .any(|b| a.file == b.file && a.segment == b.segment)
                })
            {
                a.name = format!("{} / {}", a.name, b.name);
                a.samples.append(&mut b.samples);
                a.source_aliases = a
                    .sources()
                    .chain(b.sources())
                    .filter(|source| *source != &a.source)
                    .cloned()
                    .sorted()
                    .dedup()
                    .collect();
                a.evidence = "fixedTimeAlternative".into();
            }
        }
    }
    slots.retain(|slot| !slot.samples.is_empty());
    for slot in &mut slots {
        let phase = phases
            .iter()
            .find(|phase| phase.id == slot.phase)
            .context("Missing slot phase")?;
        slot.relative_ms = (statistics(
            slot.samples
                .iter()
                .map(|sample| sample.relative_ms)
                .collect(),
        )?
        .median_ms
            / 100.0)
            .round() as i64
            * 100;
        slot.at_ms = (phase.at_ms + slot.relative_ms).max(0);
        slot.ability_ids = slot
            .samples
            .iter()
            .flat_map(|sample| {
                inputs
                    .iter()
                    .find(|input| input.pull.file == sample.file)
                    .into_iter()
                    .flat_map(|input| {
                        sample
                            .event_indices
                            .iter()
                            .filter_map(|&index| input.log.events.get(index)?.ability_game_id)
                    })
            })
            .sorted()
            .dedup()
            .collect();
        let observed: BTreeSet<_> = slot
            .samples
            .iter()
            .map(|sample| (&sample.file, sample.segment))
            .collect();
        let (phase_id, relative_ms) = (slot.phase, slot.relative_ms);
        let reached = spans
            .iter()
            .zip(inputs)
            .flat_map(|(spans, input)| {
                spans.iter().filter(move |span| {
                    let end = super::multi::boss_defeat(&input.pull, span.start_ms, span.end_ms)
                        .unwrap_or(span.end_ms);
                    span.phase == phase_id && span.origin_ms + relative_ms <= end
                })
            })
            .count();
        // Show frequent effects but require only invariant rows, e.g. conditional Holy is still optional.
        slot.predictive =
            slot.relative_ms >= 0 && observed.len() >= 2 && observed.len() * 5 >= reached * 4;
        slot.required = slot.predictive && observed.len() == reached;
        if !slot.predictive {
            slot.evidence = "conditionalReference".into();
        }
    }
    for phase in &phases {
        let samples: Vec<_> = inputs
            .iter()
            .zip(&spans)
            .flat_map(|(input, spans)| {
                spans.iter().enumerate().filter_map(move |(segment, span)| {
                    let index = span.opening?;
                    let raw = input.log.events.get(index)?;
                    (span.phase == phase.id).then(|| Sample {
                        file: input.pull.file.clone(),
                        segment,
                        event_indices: vec![index],
                        instance_ids: Vec::new(),
                        ability_id: raw.ability_game_id.unwrap_or(phase.ability),
                        time_ms: span.origin_ms,
                        block_entry_ms: span.origin_ms,
                        relative_ms: 0,
                    })
                })
            })
            .collect();
        slots.push(Slot {
            id: 0,
            phase: phase.id,
            source: phase.source.clone(),
            source_aliases: Vec::new(),
            name: "--sync--".into(),
            at_ms: phase.at_ms,
            relative_ms: 0,
            ability_ids: vec![phase.ability],
            samples,
            evidence: "phaseEntry".into(),
            selector: true,
            predictive: false,
            required: true,
            sync_enabled: true,
        });
    }
    slots.sort_by_key(|slot| {
        (
            slot.at_ms,
            !slot.selector,
            slot.source.clone(),
            slot.name.clone(),
        )
    });
    let mut entries = vec![Entry::Note { text: "Phase schedules: fixed-time alternatives share a mechanic row; conditional trace evidence is hidden from prediction.".into() }];
    for (i, slot) in slots.iter_mut().enumerate() {
        slot.id = i;
        let mut window = [2.5, 2.5];
        if slot.selector {
            window[0] = slot.at_ms as f64 / 1000.0;
        }
        entries.push(event(
            slot,
            slot.selector || slot.predictive,
            window,
            slot.selector,
        )?);
    }
    // Packet copies and nearby repeated IDs need one clock owner, e.g. Hyperdrive ticks must not cross-match.
    for (i, a) in slots.iter().enumerate().filter(|(_, slot)| !slot.selector) {
        let collision = inputs.iter().zip(&spans).any(|(input, spans)| {
            spans
                .iter()
                .filter(|span| span.phase == a.phase)
                .any(|span| {
                    input.log.events.iter().enumerate().any(|(index, raw)| {
                        input.log.report.fights.first().is_some_and(|fight| {
                            raw.kind == "cast"
                                && raw
                                    .ability_game_id
                                    .is_some_and(|id| a.ability_ids.contains(&id))
                                && a.accepts_source(
                                    raw.source_id.and_then(|id| input.actors.get(&id)),
                                )
                                && span.start_ms <= raw.timestamp - fight.start_time
                                && raw.timestamp - fight.start_time <= span.end_ms
                                && (raw.timestamp - fight.start_time - span.origin_ms)
                                    .abs_diff(a.relative_ms)
                                    < 2500
                                && !a.samples.iter().any(|sample| {
                                    sample.file == input.pull.file
                                        && sample.event_indices.contains(&index)
                                })
                        })
                    })
                })
        });
        // Keep forecasts while giving the clock to the later row, e.g. an All Things Ending packet follows Path of Light.
        let reordered = slots
            .iter()
            .filter(|b| !b.selector && b.phase == a.phase && b.at_ms > a.at_ms)
            .filter(|b| (251..=1500).contains(&b.at_ms.abs_diff(a.at_ms)))
            .any(|b| {
                a.samples.iter().any(|sample| {
                    b.samples.iter().any(|other| {
                        sample.file == other.file
                            && sample.segment == other.segment
                            && sample.relative_ms.abs_diff(other.relative_ms) <= 250
                    })
                })
            });
        if (collision || reordered)
            && let Some(Entry::Event {
                sync: Some(Sync::Network(sync)),
                ..
            }) = entries.get_mut(i + 1)
        {
            sync.enabled = false;
        }
    }
    for (slot, entry) in slots.iter_mut().zip(entries.iter().skip(1)) {
        slot.sync_enabled =
            matches!(entry, Entry::Event { sync: Some(Sync::Network(sync)), .. } if sync.enabled);
    }
    entries.push(Entry::AbilityCatalog {
        abilities: inputs
            .iter()
            .flat_map(|input| &input.catalog)
            .unique_by(|ability| &ability.id)
            .cloned()
            .collect(),
        phase: None,
    });
    let mut cursor = slots.iter();
    let mut previous_phase = None;
    let mut annotated = Vec::new();
    for entry in entries {
        if matches!(entry, Entry::Event { .. }) {
            let slot = cursor.next().context("Missing annotated slot")?;
            if previous_phase != Some(slot.phase) {
                annotated.push(Entry::Note {
                    text: format!(
                        "Phase {}: times after the observed opening cast.",
                        slot.phase
                    ),
                });
                previous_phase = Some(slot.phase);
            }
        }
        annotated.push(entry);
    }
    let yaml = draft::serialize_draft(annotated, vec![crate::timeline::ResetEvent::Wipe])?;
    let mut timeline: crate::timeline::Timeline = serde_saphyr::from_str(&yaml)?;
    timeline.hide_names.push("--reference--".into());
    let yaml = serde_saphyr::to_string(&timeline)?;
    crate::timeline::validate_value(serde_json::to_value(timeline)?)?;
    let coverage: Vec<_> = inputs.iter().map(|input| {
        let represented: BTreeSet<_> = slots.iter().filter(|slot| !slot.selector).flat_map(|slot| &slot.samples).filter(|sample| sample.file == input.pull.file).flat_map(|sample| sample.event_indices.iter().copied()).collect();
        let predicted: BTreeSet<_> = slots.iter().filter(|slot| slot.predictive).flat_map(|slot| &slot.samples).filter(|sample| sample.file == input.pull.file).flat_map(|sample| sample.event_indices.iter().copied()).collect();
        json!({"file": input.pull.file, "representedEventIndices": represented, "predictiveEventIndices": predicted,
            "omittedEventIndices": input.pull.occurrences.iter().filter(|row| row.kind == "cast" && !represented.contains(&row.event_index)).map(|row| row.event_index).collect::<Vec<_>>()})
    }).collect();
    let report = json!({"status":"draft", "mode":"raid", "group":key,
        "inputs":inputs.iter().map(|input| &input.report).collect::<Vec<_>>(), "slots":slots,
        "blocks":[], "syncConflicts":[], "omittedSignals":[], "observedPaths":inputs.iter().map(|input| json!({"file":input.pull.file,"occurrences":input.pull.occurrences})).collect::<Vec<_>>(),
        "outputCoverage":coverage, "alignment":{"implementation":"phase-relative mechanic schedules", "orderSensitivePairs":[]},
        "schedule":{"phases":phases, "segments": inputs.iter().zip(&spans).map(|(input, spans)| json!({"file":input.pull.file,"spans":spans})).collect::<Vec<_>>()},
        "extensions":{"accepted":true,"branches":[],"phases":phases,"lookaheadMs":(lookahead*1000.0).round() as i64},
        "repeats":{"accepted":false,"reason":"Phase schedules retain finite rounds."},
        "limitations":["Conditional trace evidence is retained but hidden from prediction.","Phase transitions require observed boss cast starts; ACT reset capture remains unverified."],
        "validation":{"schemaAndSemantic":true,"replay":false,"cactbotParser":false,"runtime":false}});
    Ok(Some((yaml, report)))
}

pub(super) fn evidence(
    yaml: &str,
    report: &Value,
    pull: &Pull,
    log: &CollectedLog,
) -> Result<crate::timeline::replay::Evidence> {
    let phases: Vec<Phase> = serde_json::from_value(
        report
            .pointer("/schedule/phases")
            .context("Missing schedule phases")?
            .clone(),
    )?;
    let spans = segments(pull, log, &phases)?;
    let slots: Vec<Slot> = serde_json::from_value(report["slots"].clone())?;
    let timeline: crate::timeline::Timeline = serde_saphyr::from_str(yaml)?;
    let entries: Vec<_> = timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| matches!(entry, Entry::Event { .. }) && !entry.is_combat_start())
        .map(|(i, _)| i)
        .collect();
    ensure!(entries.len() == slots.len(), "Schedule row count differs");
    let actors = actor_names(log);
    let start = log
        .report
        .fights
        .first()
        .context("Missing schedule replay fight")?
        .start_time;
    let mut result = crate::timeline::replay::Evidence::default();
    for (&entry, slot) in entries.iter().zip(&slots) {
        let mut expected = BTreeSet::new();
        let direct: Vec<_> = slot
            .samples
            .iter()
            .filter(|sample| super::replay::same_file(&sample.file, &pull.file))
            .collect();
        for span in spans.iter().filter(|span| span.phase == slot.phase) {
            if slot.selector {
                expected.extend(span.opening);
                continue;
            }
            for (index, raw) in log.events.iter().enumerate() {
                let at = raw.timestamp - start;
                if raw.kind == "cast"
                    && span.start_ms <= at
                    && at <= span.end_ms
                    && slot.accepts_source(raw.source_id.and_then(|id| actors.get(&id)))
                    && raw
                        .ability_game_id
                        .is_some_and(|id| slot.ability_ids.contains(&id))
                    // Trace-only effects have no learned clock, e.g. a player-triggered Death Bomb can resolve early.
                    && (!slot.predictive || (at - span.origin_ms).abs_diff(slot.relative_ms) < 2500)
                {
                    // The closest same-ID row owns the event, e.g. adjacent two-second tankbuster ticks stay distinct.
                    let nearest = slots
                        .iter()
                        .filter(|other| {
                            !other.selector
                                && other.phase == slot.phase
                                && other
                                    .accepts_source(raw.source_id.and_then(|id| actors.get(&id)))
                                && raw
                                    .ability_game_id
                                    .is_some_and(|id| other.ability_ids.contains(&id))
                        })
                        .min_by_key(|other| {
                            ((at - span.origin_ms).abs_diff(other.relative_ms), other.id)
                        });
                    if nearest.is_some_and(|other| other.id == slot.id) {
                        expected.insert(index);
                    }
                }
            }
        }
        if !direct.is_empty() {
            // Keep recorded assignments, e.g. a delayed training copy cannot migrate to its next tick.
            expected = direct
                .iter()
                .flat_map(|sample| sample.event_indices.iter().copied())
                .collect();
        }
        if !expected.is_empty() {
            result.observed_packets.insert(entry, expected.clone());
        }
        if slot.sync_enabled && !slot.selector {
            // The consumer spends a row after its first packet, e.g. eight targets produce one sync.
            expected = spans
                .iter()
                .filter(|span| span.phase == slot.phase)
                .filter_map(|span| {
                    expected
                        .iter()
                        .copied()
                        .filter(|&index| {
                            log.events.get(index).is_some_and(|raw| {
                                span.start_ms <= raw.timestamp - start
                                    && raw.timestamp - start <= span.end_ms
                            })
                        })
                        .min_by_key(|&index| {
                            log.events.get(index).map(|raw| (raw.timestamp, index))
                        })
                })
                .collect();
        }
        if !expected.is_empty() {
            result.expected.insert(entry, expected);
        } else if !slot.required {
            result.inactive.insert(entry);
        } else if spans
            .iter()
            .filter(|span| span.phase == slot.phase)
            .all(|span| span.origin_ms + slot.relative_ms > span.end_ms)
        {
            result.censored.insert(entry);
        } else {
            result.missing.insert(entry);
        }
        // The schedule orders mechanics, not same-wave packet arrivals, e.g. Chaos and Exdeath reverse by 20ms.
        if slots.iter().any(|other| {
            other.id != slot.id
                && other.phase == slot.phase
                && other.at_ms.abs_diff(slot.at_ms) <= 250
        }) {
            result.unordered.insert(entry);
        }
    }
    result.lookahead_ms = report
        .pointer("/extensions/lookaheadMs")
        .and_then(Value::as_i64);
    Ok(result)
}

pub(super) fn reset_indices(
    report: &Value,
    pull: &Pull,
    log: &CollectedLog,
) -> Result<BTreeSet<usize>> {
    let phases = serde_json::from_value::<Vec<Phase>>(
        report
            .pointer("/schedule/phases")
            .context("Missing schedule phases")?
            .clone(),
    )?;
    Ok(segments(pull, log, &phases)?
        .iter()
        .filter_map(|span| span.reset)
        .collect())
}

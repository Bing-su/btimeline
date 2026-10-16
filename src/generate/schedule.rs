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
    #[serde(default)]
    inferred: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    cue_times_ms: Vec<i64>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alignment_ms: Option<i64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    id: usize,
    phase: i64,
    source: String,
    #[serde(default)]
    role: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    burst_offsets_ms: Vec<i64>,
    #[serde(default)]
    cast_started: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_window_ms: Option<[i64; 2]>,
}

impl Slot {
    fn alignment_ms(&self) -> Result<i64> {
        let at = |sample: &Sample| sample.alignment_ms.unwrap_or(sample.relative_ms);
        // A compacted burst retains its onset clock, e.g. x5 must not align to its middle hit.
        let times = if self.burst_offsets_ms.is_empty() {
            self.samples.iter().map(at).collect()
        } else {
            self.samples
                .iter()
                .into_group_map_by(|sample| (&sample.file, sample.segment))
                .into_values()
                .filter_map(|samples| samples.into_iter().map(at).min())
                .collect()
        };
        Ok(statistics(times)?.median_ms.round() as i64)
    }

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

fn boss_slot(slot: &Slot, inputs: &[Input]) -> bool {
    slot.samples.iter().any(|sample| {
        inputs
            .iter()
            .find(|input| input.pull.file == sample.file)
            .is_some_and(|input| {
                input.pull.occurrences.iter().any(|row| {
                    sample.event_indices.contains(&row.event_index) && row.role.ends_with("/Boss")
                })
            })
    })
}

fn cue_clock(cues: &[i64], phase: &Phase, span: &Segment, at: i64) -> i64 {
    if phase.inferred
        && let Some((position, &observed)) =
            cues.iter().enumerate().rev().find(|(position, observed)| {
                **observed <= at && *position < phase.cue_times_ms.len()
            })
        && let Some(&expected) = phase.cue_times_ms.get(position)
    {
        return at - observed + expected;
    }
    at - span.origin_ms
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
    if let [phase] = phases
        && phase.inferred
    {
        // A stable unphased fight has one clock, e.g. permuted attack IDs do not create phase boundaries.
        boundaries = BTreeMap::from([(0, phase.id)]);
    }
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
        alignment_ms: None,
    }
}

fn event(slot: &Slot, enabled: bool, window: [f64; 2], jump: bool) -> Result<Entry> {
    let window = slot
        .sync_window_ms
        .map(|w| w.map(|ms| ms as f64 / 1000.0))
        .unwrap_or(window);
    Ok(Entry::Event {
        at: slot.at_ms as f64 / 1000.0,
        name: if slot.selector {
            "--sync--".into()
        } else {
            slot.name.clone()
        },
        duration: slot.burst_offsets_ms.last().map(|ms| *ms as f64 / 1000.0),
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
        note: None,
    })
}

// Spawn cues bound overlapping waves, e.g. six exaflare waves each have six hits even when their endpoints coincide.
fn cue_bursts(mut slots: Vec<Slot>, inputs: &[Input]) -> Result<Vec<Slot>> {
    let mut families: BTreeMap<_, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (i, slot) in slots.iter().enumerate().filter(|(_, slot)| !slot.selector) {
        let family = families
            .entry((slot.phase, slot.source.clone(), slot.name.clone()))
            .or_default();
        if slot.cast_started {
            family.0.push(i);
        } else {
            family.1.push(i);
        }
    }
    let mut removed = BTreeSet::new();
    let mut added = Vec::new();
    for (cues, effects) in families.into_values() {
        if cues.len() < 2 || effects.len() < 3 {
            continue;
        }
        let first_cue = slots
            .get(*cues.first().context("Missing spawn cue")?)
            .context("Missing cue slot")?;
        let first_effect = slots
            .get(*effects.first().context("Missing effect")?)
            .context("Missing effect slot")?;
        let intervals = cues
            .windows(2)
            .map(|pair| match pair {
                [a, b] => Ok(slots.get(*b).context("Missing next cue")?.relative_ms
                    - slots.get(*a).context("Missing previous cue")?.relative_ms),
                _ => anyhow::bail!("Invalid cue interval"),
            })
            .collect::<Result<Vec<_>>>()?;
        let period = statistics(intervals.clone())?.median_ms.round() as i64;
        let delay = first_effect.relative_ms - first_cue.relative_ms;
        let cohort = |slot: &Slot| {
            slot.samples
                .iter()
                .map(|s| (s.file.clone(), s.segment))
                .collect::<BTreeSet<_>>()
        };
        let valid = period > 500
            && delay >= 0
            && delay < period / 2
            && intervals.iter().all(|gap| gap.abs_diff(period) <= 250)
            && effects.iter().all(|&i| {
                slots.get(i).is_some_and(|s| {
                    s.required
                        && !s.sync_enabled
                        && s.ability_ids == first_effect.ability_ids
                        && cohort(s) == cohort(first_effect)
                })
            })
            && cues.iter().all(|&i| {
                slots.get(i).is_some_and(|s| {
                    s.required
                        && s.ability_ids
                            .iter()
                            .all(|id| !first_effect.ability_ids.contains(id))
                })
            });
        if !valid {
            continue;
        }
        let mut bursts = Vec::new();
        let mut covered = BTreeSet::new();
        for &cue in &cues {
            let onset = slots.get(cue).context("Missing burst cue")?.relative_ms + delay;
            let hits: Vec<_> = effects
                .iter()
                .copied()
                .filter(|&i| {
                    slots.get(i).is_some_and(|s| {
                        onset - 200 <= s.relative_ms && s.relative_ms <= onset + period + 200
                    })
                })
                .collect();
            if hits.len() < 3 {
                break;
            }
            let mut burst = slots
                .get(*hits.first().context("Missing first hit")?)
                .context("Missing hit slot")?
                .clone();
            burst.samples.clear();
            burst.burst_offsets_ms.clear();
            for &i in &hits {
                let hit = slots.get(i).context("Missing burst hit")?;
                burst
                    .burst_offsets_ms
                    .push(hit.relative_ms - burst.relative_ms);
                burst.samples.extend(hit.samples.iter().cloned());
                covered.insert(i);
            }
            burst.name = format!("{} x{}", burst.name, hits.len());
            burst.evidence = "fixedBurst".into();
            bursts.push(burst);
        }
        if bursts.len() != cues.len()
            || covered.len() != effects.len()
            || bursts.iter().any(|s| {
                s.burst_offsets_ms.len() != bursts.first().map_or(0, |s| s.burst_offsets_ms.len())
                    || s.burst_offsets_ms
                        .last()
                        .is_none_or(|last| last.abs_diff(period) > 400)
            })
        {
            continue;
        }
        for cue in cues {
            let slot = slots.get_mut(cue).context("Missing retained cue")?;
            let boss = slot.samples.iter().any(|s| {
                inputs
                    .iter()
                    .find(|input| input.pull.file == s.file)
                    .is_some_and(|input| {
                        input.pull.occurrences.iter().any(|row| {
                            s.event_indices.contains(&row.event_index)
                                && row.role.ends_with("/Boss")
                        })
                    })
            });
            if boss {
                slot.name = format!("{} (castbar)", slot.name);
            } else {
                slot.name = "--sync--".into();
                slot.predictive = false;
                slot.evidence = "burstTimingCue".into();
                slot.sync_window_ms = Some([period / 200 * 100; 2]);
            }
        }
        removed.extend(covered);
        added.extend(bursts);
    }
    let mut kept: Vec<_> = slots
        .into_iter()
        .enumerate()
        .filter(|(i, _)| !removed.contains(i))
        .map(|(_, s)| s)
        .collect();
    kept.extend(added);
    kept.sort_by(|a, b| {
        (a.at_ms, !a.selector, &a.source, &a.name).cmp(&(b.at_ms, !b.selector, &b.source, &b.name))
    });
    for (id, slot) in kept.iter_mut().enumerate() {
        slot.id = id;
    }
    Ok(kept)
}

// Free forecast bars for distinct mechanics, e.g. four unsynced 0.8s hits become one x4 bar with a duration.
fn compact_bursts(slots: Vec<Slot>, entries: Vec<Entry>) -> Result<(Vec<Slot>, Vec<Entry>)> {
    ensure!(entries.len() == slots.len() + 1, "Missing burst event rows");
    let mut entries = entries.into_iter();
    let mut compact_entries = vec![entries.next().context("Missing schedule note")?];
    let mut compact_slots = Vec::new();
    let mut rows = slots.into_iter().zip(entries).peekable();
    while let Some(first) = rows.next() {
        let mut run = vec![first];
        while rows.peek().is_some_and(|(next, _)| {
            run.last().is_some_and(|(last, _)| {
                !last.selector
                    && !next.selector
                    && !last.sync_enabled
                    && !next.sync_enabled
                    && last.burst_offsets_ms.is_empty()
                    && next.burst_offsets_ms.is_empty()
                    && last.required
                    && next.required
                    && last.phase == next.phase
                    && last.source == next.source
                    && last.source_aliases == next.source_aliases
                    && last.name == next.name
                    && last.ability_ids == next.ability_ids
                    && (1..=2500).contains(&(next.relative_ms - last.relative_ms))
                    && last
                        .samples
                        .iter()
                        .map(|s| (&s.file, s.segment))
                        .collect::<BTreeSet<_>>()
                        == next
                            .samples
                            .iter()
                            .map(|s| (&s.file, s.segment))
                            .collect::<BTreeSet<_>>()
            })
        }) {
            run.push(rows.next().context("Missing burst continuation")?);
        }
        if run.len() >= 3 {
            let count = run.len();
            let (mut slot, mut entry) = run.remove(0);
            slot.burst_offsets_ms.push(0);
            for (mut other, _) in run {
                slot.burst_offsets_ms
                    .push(other.relative_ms - slot.relative_ms);
                slot.samples.append(&mut other.samples);
            }
            slot.name = format!("{} x{count}", slot.name);
            slot.evidence = "fixedBurst".into();
            if let Entry::Event { name, duration, .. } = &mut entry {
                *name = slot.name.clone();
                *duration = slot.burst_offsets_ms.last().map(|ms| *ms as f64 / 1000.0);
            }
            compact_slots.push(slot);
            compact_entries.push(entry);
        } else {
            for (slot, entry) in run {
                compact_slots.push(slot);
                compact_entries.push(entry);
            }
        }
    }
    for (id, slot) in compact_slots.iter_mut().enumerate() {
        slot.id = id;
    }
    Ok((compact_slots, compact_entries))
}

// Require independent, matching cast-cue prefixes before inferring a clock without FFLogs phases.
fn stable_cast_cues(inputs: &[Input]) -> bool {
    if inputs
        .iter()
        .map(|input| &input.pull.report)
        .unique()
        .count()
        < 3
    {
        return false;
    }
    let cues: Vec<Vec<_>> = inputs
        .iter()
        .map(|input| {
            input
                .pull
                .occurrences
                .iter()
                .filter(|row| row.kind == "begincast" && row.role.ends_with("/Boss"))
                .sorted_by_key(|row| (row.relative_ms, row.event_index))
                .collect()
        })
        .collect();
    let Some(reference) = cues.iter().max_by_key(|rows| rows.len()) else {
        return false;
    };
    let Some(first) = reference.first() else {
        return false;
    };
    inputs.iter().zip(&cues).all(|(input, rows)| {
        let Some(opening) = rows.first() else {
            return false;
        };
        rows.len() >= 3
            && opening.ability_id == first.ability_id
            && rows
                .iter()
                .all(|row| row.actor_game_id == first.actor_game_id)
            && rows.iter().filter_map(|row| row.instance).unique().count() <= 1
            && !input
                .log
                .events
                .iter()
                .any(|event| event.kind == "zonechange")
            && rows.iter().zip(reference.iter()).all(|(row, expected)| {
                (row.relative_ms - opening.relative_ms)
                    .abs_diff(expected.relative_ms - first.relative_ms)
                    <= 2500
            })
            && rows.len()
                >= reference
                    .iter()
                    .take_while(|expected| {
                        expected.relative_ms - first.relative_ms + opening.relative_ms
                            <= input.pull.end_ms - 2500
                    })
                    .count()
    })
}

// A constant multiset proves a permutation family; require boss evidence before inferring its clock.
#[expect(
    clippy::indexing_slicing,
    reason = "Family indices originate from this slice and slot fields change without resizing it"
)]
fn generalize_permutations(slots: &mut [Slot], inputs: &[Input]) -> bool {
    let mut pending: BTreeSet<_> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.required && !slot.selector)
        .map(|(index, _)| index)
        .collect();
    let mut boss_permutation = false;
    while let Some(first) = pending.pop_first() {
        let mut family = BTreeSet::from([first]);
        let mut ids: BTreeSet<_> = slots[first].ability_ids.iter().copied().collect();
        loop {
            let peers: Vec<_> = pending
                .iter()
                .copied()
                .filter(|&index| {
                    slots[index].phase == slots[first].phase
                        && slots[index].source == slots[first].source
                        && slots[index].source_aliases == slots[first].source_aliases
                        && slots[index].role == slots[first].role
                        && slots[index].cast_started == slots[first].cast_started
                        && slots[index].ability_ids.iter().any(|id| ids.contains(id))
                })
                .collect();
            if peers.is_empty() {
                break;
            }
            for index in peers {
                pending.remove(&index);
                family.insert(index);
                ids.extend(slots[index].ability_ids.iter().copied());
            }
        }
        if ids.len() < 2
            || family.len() < 2
            || !family
                .iter()
                .any(|&index| slots[index].ability_ids.len() > 1)
        {
            continue;
        }
        let counts: Vec<BTreeMap<_, usize>> = inputs
            .iter()
            .map(|input| {
                let mut counts = BTreeMap::new();
                for sample in family
                    .iter()
                    .flat_map(|&index| &slots[index].samples)
                    .filter(|sample| sample.file == input.pull.file)
                {
                    *counts.entry(sample.ability_id).or_default() += 1;
                }
                counts
            })
            .collect();
        if counts.iter().any(|counts| counts.is_empty())
            || counts.windows(2).any(|pair| pair[0] != pair[1])
        {
            continue;
        }
        let name = inputs
            .iter()
            .flat_map(|input| &input.log.report.master_data.abilities)
            .filter(|ability| ids.contains(&ability.game_id))
            .map(|ability| ability.name.clone())
            .sorted()
            .dedup()
            .join(" / ");
        boss_permutation |= family.iter().any(|&index| boss_slot(&slots[index], inputs));
        for index in family {
            slots[index].ability_ids = ids.iter().copied().collect();
            slots[index].name = name.clone();
            slots[index].evidence = "fixedTimePermutation".into();
        }
    }
    boss_permutation
}

pub(super) fn build(
    key: &GroupKey,
    inputs: &[Input],
    lookahead: f64,
) -> Result<Option<(String, Value)>> {
    let inferred =
        inputs.iter().all(|input| input.pull.phase_spans.is_empty()) && stable_cast_cues(inputs);
    if !inferred && inputs.iter().any(|input| input.pull.phase_spans.is_empty()) {
        return Ok(None);
    }
    let spans: Vec<_> = inputs
        .iter()
        .map(|input| {
            if inferred {
                vec![(0, 0, input.pull.end_ms)]
            } else {
                input.pull.phase_spans.clone()
            }
        })
        .collect();
    let ids: Vec<_> = spans
        .iter()
        .max_by_key(|spans| spans.len())
        .into_iter()
        .flatten()
        .map(|&(id, _, _)| id)
        .unique()
        .collect();
    if ids.len() < 2 && !inferred {
        return Ok(None);
    }
    // A fixed schedule requires one observed phase order, e.g. an alternate phase graph keeps the path compiler.
    for spans in &spans {
        let positions: Option<Vec<_>> = spans
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
        for (input, spans) in inputs.iter().zip(&spans) {
            for &(_, lo, hi) in spans.iter().filter(|&&(phase, _, _)| phase == id) {
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
            inferred,
            cue_times_ms: Vec::new(),
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
    let cue_times: Vec<Vec<_>> = inputs
        .iter()
        .map(|input| {
            input
                .pull
                .occurrences
                .iter()
                .filter(|row| row.kind == "begincast" && row.role.ends_with("/Boss"))
                .map(|row| row.relative_ms)
                .sorted()
                .collect()
        })
        .collect();
    let cue_reference = cue_times.iter().max_by_key(|cues| cues.len());
    if inferred {
        let reference = cue_reference.context("Missing inferred clock")?;
        let opening = reference.first().context("Missing inferred opening")?;
        phases
            .first_mut()
            .context("Missing inferred phase")?
            .cue_times_ms = reference.iter().map(|at| at - opening).collect();
    }
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
                let mut observed = sample(input, segment, span.origin_ms, signal);
                if inferred {
                    // Match rounds on the preceding cast cue while retaining actual times for rendering and replay.
                    let cues = cue_times.get(i).context("Missing inferred cast cues")?;
                    let phase = phases
                        .iter()
                        .find(|phase| phase.id == span.phase)
                        .context("Missing inferred phase")?;
                    observed.alignment_ms = Some(cue_clock(cues, phase, span, signal.time_ms));
                }
                observations.push((
                    span.phase,
                    source,
                    name,
                    signal.key.role.clone(),
                    signal.event_indices.iter().any(|index| {
                        input.pull.occurrences.iter().any(|row| {
                            row.event_index == *index && row.start_timestamp_ms.is_some()
                        })
                    }),
                    observed,
                ));
            }
        }
    }
    // Compare borrowed fields so ordering observations does not repeatedly allocate caster/file names.
    observations.sort_by(|a, b| {
        let (phase, source, name, role, started, sample) = a;
        let (other_phase, other_source, other_name, other_role, other_started, other_sample) = b;
        (
            phase,
            source,
            name,
            role,
            started,
            sample.alignment_ms.unwrap_or(sample.relative_ms),
            &sample.file,
            sample.segment,
        )
            .cmp(&(
                other_phase,
                other_source,
                other_name,
                other_role,
                other_started,
                other_sample
                    .alignment_ms
                    .unwrap_or(other_sample.relative_ms),
                &other_sample.file,
                other_sample.segment,
            ))
    });
    let mut slots: Vec<Slot> = Vec::new();
    for (phase, source, name, role, started, sample) in observations {
        let found = slots.iter_mut().rev().find(|slot| {
            slot.phase == phase
                && slot.source == source
                && slot.name == name
                && slot.role == role
                && slot.cast_started == started
                && sample
                    .alignment_ms
                    .unwrap_or(sample.relative_ms)
                    .abs_diff(slot.relative_ms)
                    <= 750
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
                role,
                source_aliases: Vec::new(),
                name,
                at_ms: 0,
                relative_ms: sample.alignment_ms.unwrap_or(sample.relative_ms),
                ability_ids: Vec::new(),
                samples: vec![sample],
                evidence: "phaseSchedule".into(),
                selector: false,
                predictive: true,
                required: false,
                sync_enabled: false,
                burst_offsets_ms: Vec::new(),
                cast_started: started,
                sync_window_ms: None,
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
                || a.cast_started != b.cast_started
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
            // Same-named boss castbars and helper hits can overlap after drift; shared IDs prove aliases.
            if a.role != b.role && !shared_id {
                continue;
            }
            let mut times = a
                .samples
                .iter()
                .chain(&b.samples)
                .map(|sample| sample.alignment_ms.unwrap_or(sample.relative_ms));
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
                && a.role == b.role
                && a.cast_started == b.cast_started
                && a.alignment_ms()?.abs_diff(b.alignment_ms()?) <= 250
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
        // Repeated attempts in one file are not independent support, e.g. a merged retry cannot vote twice alone.
        slot.predictive = slot.relative_ms >= 0
            && slot
                .samples
                .iter()
                .map(|sample| &sample.file)
                .unique()
                .count()
                >= 2
            && observed.len() * 5 >= reached * 4;
        slot.required = slot.predictive && observed.len() == reached;
        if !slot.predictive {
            slot.evidence = "conditionalReference".into();
        }
    }
    // A conditional reaction is evidence rather than a forecast, e.g. a missed tower must not become a timeline bar.
    let (mut slots, excluded): (Vec<_>, Vec<_>) =
        slots.into_iter().partition(|slot| slot.predictive);
    if inferred {
        let common_end = spans
            .iter()
            .flatten()
            .map(|span| span.end_ms - span.origin_ms)
            .min()
            .unwrap_or(0);
        // Keep genuinely variable boss paths in the path compiler; only a censored suffix may lack support.
        if excluded.iter().any(|slot| {
            slot.cast_started && slot.relative_ms <= common_end && boss_slot(slot, inputs)
        }) || !generalize_permutations(&mut slots, inputs)
        {
            return Ok(None);
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
                        alignment_ms: None,
                    })
                })
            })
            .collect();
        slots.push(Slot {
            id: 0,
            phase: phase.id,
            source: phase.source.clone(),
            role: "NPC/Boss".into(),
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
            burst_offsets_ms: Vec::new(),
            cast_started: true,
            sync_window_ms: None,
        });
    }
    slots.sort_by(|a, b| {
        (a.at_ms, !a.selector, &a.source, &a.name).cmp(&(b.at_ms, !b.selector, &b.source, &b.name))
    });
    let mut entries = vec![Entry::Note { text: "Phase schedules: fixed-time alternatives share a mechanic row; conditional reactions are excluded and documented in the generation report.".into() }];
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
    let slots = cue_bursts(slots, inputs)?;
    let mut rebuilt = vec![
        entries
            .into_iter()
            .next()
            .context("Missing schedule note")?,
    ];
    for slot in &slots {
        rebuilt.push(event(
            slot,
            slot.sync_enabled,
            [
                if slot.selector {
                    slot.at_ms as f64 / 1000.0
                } else {
                    2.5
                },
                2.5,
            ],
            slot.selector,
        )?);
    }
    let (slots, mut entries) = compact_bursts(slots, rebuilt)?;
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
    let coverage: Vec<_> = inputs.iter().map(|input| {
        let represented: BTreeSet<_> = slots.iter().filter(|slot| !slot.selector).flat_map(|slot| &slot.samples).filter(|sample| sample.file == input.pull.file).flat_map(|sample| sample.event_indices.iter().copied()).collect();
        let predicted: BTreeSet<_> = slots.iter().filter(|slot| slot.predictive).flat_map(|slot| &slot.samples).filter(|sample| sample.file == input.pull.file).flat_map(|sample| sample.event_indices.iter().copied()).collect();
        let excluded: BTreeSet<_> = excluded.iter().flat_map(|slot| &slot.samples).filter(|sample| sample.file == input.pull.file).flat_map(|sample| sample.event_indices.iter().copied()).collect();
        json!({"file": input.pull.file, "representedEventIndices": represented, "predictiveEventIndices": predicted, "excludedEventIndices": excluded,
            "omittedEventIndices": input.pull.occurrences.iter().filter(|row| row.kind == "cast" && !represented.contains(&row.event_index) && !excluded.contains(&row.event_index)).map(|row| row.event_index).collect::<Vec<_>>()})
    }).collect();
    let report = json!({"status":"draft", "mode":"raid", "group":key,
        "inputs":inputs.iter().map(|input| &input.report).collect::<Vec<_>>(), "slots":slots,
        "excludedSignals":excluded.iter().map(|slot| json!({"reason":"conditionalTimingOrOccurrence", "slot":slot})).collect::<Vec<_>>(),
        "blocks":[], "syncConflicts":[], "omittedSignals":[], "observedPaths":inputs.iter().map(|input| json!({"file":input.pull.file,"occurrences":input.pull.occurrences})).collect::<Vec<_>>(),
        "outputCoverage":coverage, "alignment":{"implementation":"phase-relative mechanic schedules", "orderSensitivePairs":[]},
        "schedule":{"phases":phases, "segments": inputs.iter().zip(&spans).map(|(input, spans)| json!({"file":input.pull.file,"spans":spans})).collect::<Vec<_>>()},
        "extensions":{"accepted":true,"branches":[],"phases":phases,"lookaheadMs":(lookahead*1000.0).round() as i64},
        "repeats":{"accepted":false,"reason":"Phase schedules retain finite rounds."},
        "limitations":["Conditional or insufficiently supported effects are excluded; evidence remains in excludedSignals.","Phase transitions require observed boss cast starts; ACT reset capture remains unverified."],
        "validation":{"schemaAndSemantic":true,"replay":false,"cactbotParser":false,"runtime":false}});
    if inferred {
        for input in inputs {
            let evidence = evidence(&yaml, &report, &input.pull, &input.log)?;
            let signals = super::replay::signals(&input.log)?;
            // Cue agreement is only a candidate, e.g. a helper collision must keep the existing path compiler.
            if !crate::timeline::replay::run(&yaml, &signals, input.pull.end_ms, &evidence)?.passed
            {
                return Ok(None);
            }
        }
    }
    Ok(Some((yaml, report)))
}

// Apply learned exclusions to unseen packets too, e.g. an optional failed soak may occur earlier in a holdout.
pub(super) fn excluded_events(
    report: &Value,
    pull: &Pull,
    log: &CollectedLog,
) -> Result<BTreeSet<usize>> {
    let phases: Vec<Phase> = serde_json::from_value(
        report
            .pointer("/schedule/phases")
            .context("Missing exclusion phases")?
            .clone(),
    )?;
    let spans = segments(pull, log, &phases)?;
    let actors = actor_names(log);
    let start = log
        .report
        .fights
        .first()
        .context("Missing exclusion fight")?
        .start_time;
    let mut excluded = BTreeSet::new();
    for item in report["excludedSignals"].as_array().into_iter().flatten() {
        let slot: Slot = serde_json::from_value(item["slot"].clone())?;
        for (index, raw) in log.events.iter().enumerate() {
            let at = raw.timestamp - start;
            if raw.kind == "cast"
                && slot.accepts_source(raw.source_id.and_then(|id| actors.get(&id)))
                && raw
                    .ability_game_id
                    .is_some_and(|id| slot.ability_ids.contains(&id))
                && spans.iter().any(|span| {
                    span.phase == slot.phase && span.start_ms <= at && at <= span.end_ms
                })
            {
                excluded.insert(index);
            }
        }
    }
    Ok(excluded)
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
    let cue_times: Vec<_> = pull
        .occurrences
        .iter()
        .filter(|row| row.kind == "begincast" && row.role.ends_with("/Boss"))
        .map(|row| row.relative_ms)
        .sorted()
        .collect();
    let aligned_slots: BTreeMap<_, _> = slots
        .iter()
        .map(|slot| {
            let inferred = phases
                .iter()
                .any(|phase| phase.id == slot.phase && phase.inferred);
            Ok((
                slot.id,
                if inferred {
                    slot.alignment_ms()?
                } else {
                    slot.relative_ms
                },
            ))
        })
        .collect::<Result<_>>()?;
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
            let phase = phases
                .iter()
                .find(|phase| phase.id == span.phase)
                .context("Missing replay phase")?;
            let slot_ms = *aligned_slots
                .get(&slot.id)
                .context("Missing aligned slot")?;
            if slot.selector {
                expected.extend(span.opening);
                continue;
            }
            for (index, raw) in log.events.iter().enumerate() {
                let at = raw.timestamp - start;
                let aligned_ms = cue_clock(&cue_times, phase, span, at);
                if raw.kind == "cast"
                    && span.start_ms <= at
                    && at <= span.end_ms
                    && slot.accepts_source(raw.source_id.and_then(|id| actors.get(&id)))
                    && raw
                        .ability_game_id
                        .is_some_and(|id| slot.ability_ids.contains(&id))
                    && (aligned_ms.abs_diff(slot_ms) < 2500
                        || slot
                            .burst_offsets_ms
                            .iter()
                            .any(|offset| aligned_ms.abs_diff(slot_ms + offset) < 2500))
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
                            (
                                std::iter::once(0)
                                    .chain(other.burst_offsets_ms.iter().copied())
                                    .map(|offset| {
                                        aligned_ms.abs_diff(
                                            aligned_slots
                                                .get(&other.id)
                                                .copied()
                                                .unwrap_or(other.relative_ms)
                                                + offset,
                                        )
                                    })
                                    .min()
                                    .unwrap_or(u64::MAX),
                                other.id,
                            )
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

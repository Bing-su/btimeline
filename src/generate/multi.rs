use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use itertools::Itertools;
use path_slash::PathBufExt as _;
use serde_json::Value;

use super::alignment::{self, Signal, SignalKey};
use super::report::{
    Alignment, ConflictingEvent, MultiBlock, MultiConflict, MultiReport, MultiSlot, ObservedPath,
    OmittedSignal, OutputCoverage, Sample, SensitivePair, TimeStatistics, Validation,
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

pub(super) fn correspondence(left: &Pull, right: &Pull) -> Result<(Correspondence, bool)> {
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
    report: Value,
    pub signals: BTreeMap<usize, Signal>,
    catalog: Vec<Ability>,
    pub actors: BTreeMap<i64, String>,
}

fn prepare(file: &str, mode: GenerateMode) -> Result<Input> {
    let source = load_one(&PathBuf::from_slash(file))?;
    let draft::SingleDraft {
        catalog, report, ..
    } = draft::build_single(&source, mode)?;
    // Align the same normalized read used for raw collision checks, e.g. if a pull was recollected.
    let mut pull = source.pull;
    if mode == GenerateMode::Dungeon {
        input::filter_boss_spans(&mut pull, &report)?;
    }
    let signals = alignment::signals(&pull)
        .into_iter()
        .map(|s| Ok((index(&s)?, s)))
        .collect::<Result<_>>()?;
    let log = source.log;
    let actors = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.clone()))
        .collect();
    Ok(Input {
        pull,
        log,
        report,
        signals,
        catalog,
        actors,
    })
}

pub(super) fn build(group: Group, mode: GenerateMode, lookahead: f64) -> Result<(String, Value)> {
    let Group { key, mut pulls } = group;
    pulls.sort_by(|a, b| (&a.report, a.fight, &a.file).cmp(&(&b.report, b.fight, &b.file)));
    // Own each pull beside its raw evidence; sorting/filtering cannot desynchronize parallel lists.
    let inputs = pulls
        .into_iter()
        .map(|pull| prepare(&pull.file, mode))
        .collect::<Result<Vec<_>>>()?;
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
    for [(a, left), (b, right)] in inputs.iter().enumerate().array_combinations() {
        let (forward, order_sensitive) = correspondence(&left.pull, &right.pull)?;
        let (backward, _) = correspondence(&right.pull, &left.pull)?;
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
    for (&event_index, signal) in reference_signals
        .iter()
        .sorted_by_key(|(index, signal)| (signal.time_ms, **index))
    {
        let mut samples = Vec::new();
        let mut censored = Vec::new();
        let mut valid = true;
        for (i, input) in inputs.iter().enumerate() {
            let matched = if i == reference {
                Some(event_index)
            } else {
                relations
                    .get(&(reference, i))
                    .and_then(|r| r.matched.get(&event_index))
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
                .is_some_and(|r| r.censored.contains(&event_index))
            {
                censored.push(i);
            } else {
                valid = false;
            }
        }
        // Every observed pair must agree on the same occurrence, including repeat positions.
        for &(a, left) in &samples {
            let left_index = index(left)?;
            for &missing in &censored {
                if !relations
                    .get(&(a, missing))
                    .is_some_and(|r| r.censored.contains(&left_index))
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
                .is_some_and(|r| r.matched.get(&left_index) == Some(&right_index))
            {
                valid = false;
            }
        }
        if !valid {
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
        }
        if signal.key.kind != "cast" {
            continue;
        }
        let mut relative_times = Vec::new();
        let mut source_refs = Vec::new();
        let mut ids = BTreeSet::new();
        let mut sources = BTreeSet::new();
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
                valid = false;
                continue;
            };
            relative_times.push(sample.time_ms - entry_ms);
            ids.insert(sample.key.ability_id);
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
        if !valid {
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
        // Use fight-relative medians for every role to preserve order with the same reached pulls.
        // For A→helper→B, median(A)+median(helper−A) can exceed median(B); keep intervals in the report.
        let at_ms = absolute.median_ms;
        let at = (at_ms / 100.0).round() / 10.0;
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
            .process_results(|mut names| names.join(" / "))?;
        let patterns: Vec<_> = ids.iter().map(|id| format!("^{id:X}$")).collect();
        let mut fields = BTreeMap::new();
        fields.insert("id".into(), field_pattern(patterns)?);
        let invalid_source = sources
            .iter()
            .any(|name| name.contains(['#', '"', '\r', '\n']));
        if !invalid_source {
            let patterns: Vec<_> = sources
                .iter()
                .map(|source| format!("^{}$", regress::escape(source)))
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
            outside_window |=
                sample.is_some_and(|s| (s.time_ms as f64 - at * 1000.0).abs() > 2500.0);
            let observed_ms =
                sample.map_or(at * 1000.0, |s| draft::rounded_seconds(s.time_ms) * 1000.0);
            for (j, event) in input.log.events.iter().enumerate() {
                let source_matches = event
                    .source_id
                    .and_then(|actor| input.actors.get(&actor))
                    .is_some_and(|name| sources.contains(name));
                if event.kind == "cast"
                    && event.ability_game_id.is_some_and(|id| ids.contains(&id))
                    && source_matches
                    && Some(j) != representative
                    && ((event.timestamp - start) as f64 - at * 1000.0)
                        .abs()
                        .min(((event.timestamp - start) as f64 - observed_ms).abs())
                        <= 2500.0
                {
                    collisions.push(ConflictingEvent {
                        file: &input.pull.file,
                        event_index: j,
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
        if let Some(reason) = reason {
            conflicts.push(MultiConflict {
                slot: slots.len(),
                reason,
                conflicting_events: collisions,
            });
        }
        entries.push(Entry::Event {
            at,
            name,
            duration: None,
            jump: None,
            sync: Some(Sync::Network(NetworkSync {
                log: LogType::Ability,
                fields,
                enabled: reason.is_none(),
                window: None,
            })),
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
    let mut abilities = Vec::new();
    let mut seen = BTreeSet::new();
    for input in &inputs {
        for ability in &input.catalog {
            if seen.insert(ability.id.clone()) {
                abilities.push(ability.clone());
            }
        }
    }
    entries.push(Entry::AbilityCatalog {
        abilities,
        phase: None,
    });
    let output_coverage = inputs
        .iter()
        .map(|input| {
            let file = &input.pull.file;
            let mut represented = BTreeSet::new();
            for slot in &slots {
                for sample in &slot.samples {
                    if sample.file == file {
                        represented.extend(sample.event_indices.iter().copied());
                    }
                }
            }
            let omitted_events = input
                .signals
                .values()
                .filter(|s| s.key.kind == "cast")
                .flat_map(|s| &s.event_indices)
                .filter(|&&index| !represented.contains(&index))
                .copied()
                .collect_vec();
            OutputCoverage {
                file,
                represented_event_indices: represented,
                omitted_event_indices: omitted_events,
            }
        })
        .collect();
    let yaml = draft::serialize_draft(entries)?;
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
    super::phase::expand(yaml, report, &inputs, lookahead)
}

// Preserve scalar patterns for one value and arrays for alternatives, e.g. ^A$ versus [^A$, ^B$].
pub(super) fn field_pattern(mut patterns: Vec<String>) -> Result<FieldPattern> {
    if patterns.len() == 1 {
        Ok(FieldPattern::One(
            patterns.pop().context("Missing field pattern")?,
        ))
    } else {
        Ok(FieldPattern::Many(patterns))
    }
}

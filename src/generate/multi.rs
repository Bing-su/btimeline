use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{
    GenerateMode, Group, Pull,
    alignment::{self, Signal, SignalKey},
    draft, inspect,
};
use crate::fflogs::model::CollectedLog;

pub(super) fn select_group(
    input: &Path,
    name: Option<&str>,
    encounter: Option<i64>,
    difficulty: Option<i64>,
) -> Result<Group> {
    let mut paths = Vec::new();
    let mut pending = vec![input.to_path_buf()];
    // Do not follow directory symlinks: a log tree must not recurse through a cycle.
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir()
                    || (kind.is_file()
                        && entry.path().extension().is_some_and(|e| e == "json")
                        && !entry
                            .file_name()
                            .to_string_lossy()
                            .ends_with(".report.json"))
                {
                    pending.push(entry.path());
                }
            }
        } else {
            paths.push(path);
        }
    }
    paths.sort();
    let groups = inspect(&paths)?;
    let choices = groups
        .iter()
        .map(|group| {
            format!(
                "{} (--encounter {} --difficulty {}, {} pulls)",
                group
                    .pulls
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" / "),
                group.key.encounter,
                group.key.difficulty,
                group.pulls.len()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut selected: Vec<_> = groups
        .into_iter()
        .filter(|group| {
            name.is_none_or(|name| group.pulls.iter().any(|pull| pull.name == name))
                && encounter.is_none_or(|id| group.key.encounter == id)
                && difficulty.is_none_or(|id| group.key.difficulty == id)
        })
        .collect();
    ensure!(
        selected.len() == 1,
        "Select exactly one compatible group with --name or --encounter / --difficulty (matched {}); available: {choices}",
        selected.len()
    );
    selected.pop().context("Missing selected group")
}

#[derive(Default)]
struct Correspondence {
    matched: BTreeMap<usize, usize>,
    censored: BTreeSet<usize>,
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

fn correspondence(left: &Pull, right: &Pull) -> Result<(Correspondence, bool)> {
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

fn statistics(mut times: Vec<i64>) -> Result<Value> {
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
    Ok(
        json!({"medianMs":median, "minMs":times.first(), "maxMs":times.last(), "sampleCount":times.len()}),
    )
}

struct Input {
    log: CollectedLog,
    report: Value,
    signals: BTreeMap<usize, Signal>,
    catalog: Vec<Value>,
    actors: BTreeMap<i64, String>,
}

fn prepare(pull: &mut Pull, mode: GenerateMode) -> Result<Input> {
    let path = PathBuf::from(&pull.file);
    let (yaml, report) = draft::build_single(&path, mode)?;
    let timeline: Value = serde_saphyr::from_str(&yaml)?;
    let catalog = timeline
        .get("entries")
        .context("Missing entries")?
        .as_array()
        .context("Missing entries")?
        .iter()
        .find(|entry| entry["kind"] == "abilityCatalog")
        .context("Missing catalog")?
        .get("abilities")
        .context("Missing catalog abilities")?
        .as_array()
        .context("Missing catalog abilities")?
        .clone();
    if mode == GenerateMode::Dungeon {
        let spans = report
            .get("bossSegments")
            .context("Missing boss segments")?
            .as_array()
            .context("Missing boss segments")?;
        pull.occurrences.retain(|row| {
            spans.iter().any(|span| {
                span["startMs"]
                    .as_i64()
                    .is_some_and(|start| row.relative_ms >= start)
                    && span["endMs"]
                        .as_i64()
                        .is_some_and(|end| row.relative_ms <= end)
            })
        });
    }
    let signals = alignment::signals(pull)
        .into_iter()
        .map(|s| Ok((index(&s)?, s)))
        .collect::<Result<_>>()?;
    let log: CollectedLog = serde_json::from_slice(&fs::read(path)?)?;
    let actors = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.clone()))
        .collect();
    Ok(Input {
        log,
        report,
        signals,
        catalog,
        actors,
    })
}

pub(super) fn build(mut group: Group, mode: GenerateMode) -> Result<(String, Value)> {
    group
        .pulls
        .sort_by(|a, b| (&a.report, a.fight, &a.file).cmp(&(&b.report, b.fight, &b.file)));
    let mut inputs = Vec::new();
    for pull in &mut group.pulls {
        inputs.push(prepare(pull, mode)?);
    }
    // The longest observed path provides positions, never permission to emit its exclusive branches.
    let reference = group
        .pulls
        .iter()
        .enumerate()
        .max_by_key(|(i, pull)| (pull.occurrences.len(), std::cmp::Reverse(*i)))
        .map(|(i, _)| i)
        .context("Missing reference pull")?;
    let mut relations = BTreeMap::new();
    let mut sensitive = Vec::new();
    // ponytail: reuse P4 pair evidence; an indexed graph is appropriate only for substantially larger groups.
    for (a, left) in group.pulls.iter().enumerate() {
        for (b, right) in group.pulls.iter().enumerate().skip(a + 1) {
            let (forward, order_sensitive) = correspondence(left, right)?;
            let (backward, _) = correspondence(right, left)?;
            relations.insert((a, b), forward);
            relations.insert((b, a), backward);
            if order_sensitive {
                sensitive.push(json!({"left":left.file, "right":right.file}));
            }
        }
    }
    let reference_signals = &inputs
        .get(reference)
        .context("Missing reference input")?
        .signals;
    let mut entries = vec![json!({"kind":"note", "text":format!(
        "Draft from {} compatible FFLogs pulls; common rows and alternatives with an observed common successor. Repeats are finite; unresolved paths are omitted. No encounter transitions or replay validation.", group.pulls.len()
    )})];
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
    let mut blocks = vec![
        json!({"id":0, "entry":"fightStart", "draftEntryMs":0, "time":statistics(vec![0; inputs.len()])?}),
    ];
    let mut anchor_indices: Option<Vec<Option<usize>>> = None;
    let mut block_id = 0;
    let mut previous_at = 0.0_f64;
    let mut ordered: Vec<_> = reference_signals.iter().collect();
    ordered.sort_by_key(|(index, s)| (s.time_ms, **index));
    for (&event_index, signal) in ordered {
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
        for (position, &(a, left)) in samples.iter().enumerate() {
            let left_index = index(left)?;
            for &missing in &censored {
                if !relations
                    .get(&(a, missing))
                    .is_some_and(|r| r.censored.contains(&left_index))
                {
                    valid = false;
                }
            }
            for &(b, right) in samples.iter().skip(position + 1) {
                let right_index = index(right)?;
                if !relations
                    .get(&(a, b))
                    .is_some_and(|r| r.matched.get(&left_index) == Some(&right_index))
                {
                    valid = false;
                }
            }
        }
        if !valid {
            omitted.push(json!({"file":group.pulls.get(reference).context("Missing reference")?.file,
                "eventIndices":signal.event_indices, "reason":"no direction-independent group consensus or dependent/unresolved path", "evidence":"unknown"}));
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
            source_refs.push(json!({"file":group.pulls.get(i).context("Missing sample pull")?.file,
                "eventIndices":sample.event_indices, "instanceIds":sample.instance_ids, "abilityId":sample.key.ability_id,
                "timeMs":sample.time_ms, "blockEntryMs":entry_ms, "relativeMs":sample.time_ms - entry_ms}));
        }
        if !valid {
            omitted.push(json!({"eventIndices":signal.event_indices, "reason":"block entry unobserved", "evidence":"unknown"}));
            continue;
        }
        let timing = statistics(relative_times)?;
        let absolute = statistics(samples.iter().map(|(_, s)| s.time_ms).collect())?;
        // Use fight-relative medians for every role to preserve order with the same reached pulls.
        // For A→helper→B, median(A)+median(helper−A) can exceed median(B); keep intervals in the report.
        let at_ms = absolute
            .get("medianMs")
            .and_then(Value::as_f64)
            .context("Missing slot median")?;
        let at = (at_ms / 100.0).round() / 10.0;
        if at < previous_at {
            // A changing reached-pull subset can invert medians even though each observed path is ordered.
            omitted.push(json!({"eventIndices":signal.event_indices, "samples":source_refs, "time":timing,
                "reason":"timing medians violate order after the reached sample set changes", "evidence":"unknown"}));
            continue;
        }
        previous_at = at;
        let names = ids
            .iter()
            .map(|id| {
                catalog
                    .get(&id.to_string())
                    .and_then(Value::as_str)
                    .context("Missing catalog name")
            })
            .collect::<Result<Vec<_>>>()?;
        let patterns: Vec<_> = ids.iter().map(|id| format!("^{id:X}$")).collect();
        let mut fields = serde_json::Map::new();
        fields.insert(
            "id".into(),
            if patterns.len() == 1 {
                json!(patterns.first())
            } else {
                json!(patterns)
            },
        );
        let invalid_source = sources
            .iter()
            .any(|name| name.contains(['#', '"', '\r', '\n']));
        if !invalid_source {
            let patterns: Vec<_> = sources
                .iter()
                .map(|source| format!("^{}$", regress::escape(source)))
                .collect();
            fields.insert(
                "source".into(),
                if patterns.len() == 1 {
                    json!(patterns.first())
                } else {
                    json!(patterns)
                },
            );
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
                    collisions.push(json!({"file":group.pulls.get(i).context("Missing collision pull")?.file, "eventIndex":j}));
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
        let sync = match reason {
            Some(_) => json!({"log":"Ability", "fields":fields, "enabled":false}),
            None => json!({"log":"Ability", "fields":fields}),
        };
        let mut entry = serde_json::Map::from_iter([
            ("kind".into(), json!("event")),
            ("at".into(), json!(at)),
            ("name".into(), json!(names.join(" / "))),
            ("sync".into(), sync),
        ]);
        if let Some(reason) = reason {
            entry.insert(
                "note".into(),
                json!(format!(
                    "Sync disabled: {reason}; see slot {} in report",
                    slots.len()
                )),
            );
            conflicts
                .push(json!({"slot":slots.len(), "reason":reason, "conflictingEvents":collisions}));
        }
        entries.push(Value::Object(entry));
        slots.push(json!({"id":slots.len(), "block":block_id, "atMs":at_ms, "time":timing, "absoluteTime":absolute,
            "abilityIds":ids, "samples":source_refs, "unobservedAfterWipe":censored.iter().filter_map(|&i| group.pulls.get(i).map(|p| &p.file)).collect::<Vec<_>>(),
            "evidence":if ids.len() > 1 { "alternativeWithCommonSuccessor" } else { "observed" }}));
        if signal.key.role.ends_with("/Boss") && ids.len() == 1 {
            block_id = blocks.len();
            let mut anchors = vec![None; inputs.len()];
            for &(i, sample) in &samples {
                *anchors.get_mut(i).context("Missing anchor position")? = Some(index(sample)?);
            }
            anchor_indices = Some(anchors);
            blocks.push(
                json!({"id":block_id, "entry":"observedBossCast", "slot":slots.len() - 1,
                "draftEntryMs":at_ms, "time":absolute, "samples":source_refs}),
            );
        }
    }
    // Reuse P3's mode-filtered first-seen catalogs so unfinished casts keep their provenance too.
    let mut abilities = Vec::new();
    let mut seen = BTreeSet::new();
    for input in &inputs {
        for ability in &input.catalog {
            if seen.insert(
                ability["id"]
                    .as_str()
                    .context("Missing catalog ID")?
                    .to_owned(),
            ) {
                abilities.push(ability.clone());
            }
        }
    }
    entries.push(json!({"kind":"abilityCatalog", "abilities":abilities}));
    let output_coverage = inputs.iter().enumerate().map(|(i, input)| {
        let file = &group.pulls.get(i).context("Missing coverage pull")?.file;
        let mut represented = BTreeSet::new();
        for slot in &slots {
            for sample in slot["samples"].as_array().context("Missing samples")? {
                if sample["file"].as_str() == Some(file) {
                    for event in sample["eventIndices"].as_array().context("Missing event indices")? {
                        represented.insert(event.as_u64().context("Invalid event index")?);
                    }
                }
            }
        }
        let omitted_events = input.signals.values().filter(|s| s.key.kind == "cast")
            .flat_map(|s| &s.event_indices).filter(|&&index| !represented.contains(&(index as u64))).copied().collect::<Vec<_>>();
        Ok(json!({"file":file,"representedEventIndices":represented,"omittedEventIndices":omitted_events}))
    }).collect::<Result<Vec<_>>>()?;
    let yaml = draft::serialize_draft(entries)?;
    let report = json!({"status":"draft", "mode":mode, "group":group.key,
        "inputs":inputs.iter().map(|input| &input.report).collect::<Vec<_>>(),
        "blocks":blocks, "slots":slots, "syncConflicts":conflicts, "omittedSignals":omitted,
        "outputCoverage":output_coverage,
        "observedPaths":group.pulls.iter().map(|p| json!({"file":p.file,"occurrences":p.occurrences})).collect::<Vec<_>>(),
        "unobservedCombinations":"unknown",
        "alignment":{"implementation":"similar 3 Myers/LCS", "orderSensitivePairs":sensitive},
        "limitations":["Unresolved or dependent paths are omitted; unobserved combinations remain unknown.",
            "ID arrays require an isolated one-ability substitution and an observed common successor; multiple varying choices remain unresolved.",
            "Repeats are expanded finitely; no conditional exits or encounter transitions are inferred.",
            "Block entries are observational timing references; entry sync, replay and runtime compatibility are unverified."],
        "validation":{"schemaAndSemantic":true,"replay":false,"cactbotParser":false,"runtime":false}});
    Ok((yaml, report))
}

//! Compose observed checkpoint encounters without aligning their distinct mechanics, e.g. R12S P1 and P2.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context, Result, ensure};
use path_slash::PathBufExt as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{GenerateMode, Group, GroupKey, Pull, draft, load_one};
use crate::fflogs::model::CollectedLog;
use crate::timeline::{
    Destination,
    Entry,
    FieldPattern,
    Jump,
    JumpWhen,
    LogType,
    NetworkSync,
    Sync,
    Timeline,
};

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Opening {
    ability_id: i64,
    source: String,
    at_ms: i64,
    entry: usize,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Segment {
    group: GroupKey,
    offset_ms: i64,
    yaml: String,
    report: Value,
    entry_map: Vec<usize>,
    opening: Opening,
}

// Require evidence of a shared battle and its order; adjacent encounter numbers alone prove no relationship.
fn ordered(mut groups: Vec<Group>) -> Result<Vec<Group>> {
    let mut zone = None;
    let mut names: Option<BTreeSet<String>> = None;
    let mut before = BTreeSet::new();
    for group in &groups {
        let source = load_one(&std::path::PathBuf::from_slash(
            &group
                .pulls
                .first()
                .context("Missing checkpoint input")?
                .file,
        ))?;
        let fight = source
            .log
            .report
            .fights
            .first()
            .context("Missing checkpoint fight")?;
        let current=fight.extra.get("gameZone").and_then(|z|z.get("id")).and_then(Value::as_i64).context("Select exactly one compatible group: checkpoint composition requires a shared game zone")?;
        ensure!(
            zone.is_none_or(|z| z == current),
            "Select exactly one compatible group: game zones differ"
        );
        zone = Some(current);
        let enemies: BTreeSet<_> = fight.enemy_npcs.iter().map(|a| a.id).collect();
        let current: BTreeSet<_> = source
            .log
            .report
            .master_data
            .actors
            .iter()
            .filter(|a| a.sub_type == "Boss" && enemies.contains(&a.id))
            .map(|a| a.name.clone())
            .collect();
        names = Some(names.map_or_else(
            || current.clone(),
            |old| old.intersection(&current).cloned().collect(),
        ));
    }
    ensure!(
        names.is_some_and(|n| !n.is_empty()),
        "Select exactly one compatible group: boss identities differ"
    );
    for (i, a) in groups.iter().enumerate() {
        for b in groups.iter().skip(i + 1) {
            ensure!(
                a.key.difficulty == b.key.difficulty,
                "Select exactly one compatible group: difficulties differ"
            );
            let mut directions = BTreeSet::new();
            for report in a.pulls.iter().map(|p| &p.report).collect::<BTreeSet<_>>() {
                let first_a = a
                    .pulls
                    .iter()
                    .filter(|p| &p.report == report)
                    .map(|p| p.fight)
                    .min();
                let first_b = b
                    .pulls
                    .iter()
                    .filter(|p| &p.report == report)
                    .map(|p| p.fight)
                    .min();
                if let (Some(a), Some(b)) = (first_a, first_b) {
                    directions.insert(a < b);
                }
            }
            ensure!(
                directions.len() == 1,
                "Select exactly one compatible group: checkpoint order is unobserved or inconsistent"
            );
            before.insert(if directions.contains(&true) {
                (a.key.clone(), b.key.clone())
            } else {
                (b.key.clone(), a.key.clone())
            });
        }
    }
    groups.sort_by(|a, b| {
        if a.key == b.key {
            std::cmp::Ordering::Equal
        } else if before.contains(&(a.key.clone(), b.key.clone())) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ensure!(
        groups.iter().enumerate().all(|(i, a)| groups
            .iter()
            .skip(i + 1)
            .all(|b| before.contains(&(a.key.clone(), b.key.clone())))),
        "Checkpoint order contains a cycle"
    );
    Ok(groups)
}

fn inputs(report: &Value) -> Result<Vec<&Value>> {
    if report.get("input").is_some() {
        Ok(vec![report])
    } else {
        Ok(report
            .get("inputs")
            .and_then(Value::as_array)
            .context("Missing checkpoint inputs")?
            .iter()
            .collect())
    }
}

fn opening(report: &Value) -> Result<(Opening, Vec<Value>)> {
    let mut choices = BTreeMap::new();
    for input in inputs(report)? {
        let file = input
            .pointer("/input/file")
            .and_then(Value::as_str)
            .context("Missing checkpoint source")?;
        let source = load_one(&std::path::PathBuf::from_slash(file))?;
        let row = source
            .pull
            .occurrences
            .iter()
            .find(|r| r.kind == "begincast" && r.role.ends_with("/Boss"))
            .context("Checkpoint requires an observed boss opening cast")?;
        let actor = source
            .log
            .report
            .master_data
            .actors
            .iter()
            .find(|a| a.id == row.actor_id)
            .context("Missing checkpoint actor")?;
        choices.entry((row.ability_id,actor.name.clone())).or_insert_with(Vec::new).push(json!({"file":file,"eventIndices":[row.event_index],"timeMs":row.relative_ms,"abilityId":row.ability_id}));
    }
    ensure!(
        choices.len() == 1,
        "Checkpoint opening is not stable across selected inputs"
    );
    let ((ability_id, source), samples) = choices
        .into_iter()
        .next()
        .context("Missing checkpoint opening")?;
    let at_ms = (super::multi::statistics(
        samples
            .iter()
            .map(|s| s["timeMs"].as_i64().context("Missing opening time"))
            .collect::<Result<_>>()?,
    )?
    .median_ms
        / 100.0)
        .round() as i64
        * 100;
    Ok((
        Opening {
            ability_id,
            source,
            at_ms,
            entry: 0,
        },
        samples,
    ))
}

pub(super) fn build(
    groups: Vec<Group>,
    mode: GenerateMode,
    lookahead: f64,
) -> Result<(String, Value)> {
    ensure!(
        mode == GenerateMode::Raid,
        "Select exactly one compatible group: checkpoint composition is for raid inputs"
    );
    let groups = ordered(groups)?;
    let keys: Vec<_> = groups.iter().map(|g| g.key.clone()).collect();
    let starts: Vec<BTreeSet<_>> = groups
        .iter()
        .map(|g| {
            g.pulls
                .iter()
                .flat_map(|p| p.occurrences.iter())
                .filter(|r| r.kind == "begincast")
                .map(|r| r.ability_id)
                .collect()
        })
        .collect();
    let mut entries = Vec::new();
    let mut slots = Vec::new();
    let mut segments = Vec::new();
    let mut offset = 0i64;
    let mut catalog = BTreeSet::new();
    for (position, group) in groups.into_iter().enumerate() {
        let (yaml, mut report) = draft::build_group(group, mode, lookahead)?;
        if report.get("outputCoverage").is_none() {
            let mut represented: BTreeSet<usize> = serde_json::from_value(
                report
                    .get("emittedEventIndices")
                    .context("Missing single checkpoint evidence")?
                    .clone(),
            )?;
            for collapsed in report
                .get("collapsedCasts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                represented.extend(serde_json::from_value::<BTreeSet<usize>>(
                    collapsed
                        .get("omittedEventIndices")
                        .context("Missing collapsed checkpoint evidence")?
                        .clone(),
                )?);
            }
            let file = report
                .pointer("/input/file")
                .context("Missing checkpoint file")?
                .clone();
            super::phase::put(
                json!([{"file":file,"representedEventIndices":represented,"excludedEventIndices":[],"omittedEventIndices":[]}]),
                &mut report,
                "outputCoverage",
            )?;
        }
        let timeline: Timeline = serde_saphyr::from_str(&yaml)?;
        let (mut opening, samples) = opening(&report)?;
        ensure!(
            starts
                .iter()
                .enumerate()
                .all(|(i, ids)| i == position || !ids.contains(&opening.ability_id)),
            "Checkpoint opening can also match another encounter"
        );
        let mut map = vec![0; timeline.entries.len()];
        let mut rows = Vec::new();
        let mut local_slots = report
            .get("slots")
            .and_then(Value::as_array)
            .context("Missing checkpoint slots")?
            .iter();
        for (index, mut entry) in timeline.entries.into_iter().enumerate() {
            if entry.is_combat_start() {
                if entries.is_empty() {
                    entries.push(entry);
                }
                continue;
            }
            let mut slot = if matches!(entry, Entry::Event { .. }) {
                Some(
                    local_slots
                        .next()
                        .context("Missing checkpoint row")?
                        .clone(),
                )
            } else {
                None
            };
            match &mut entry {
                Entry::Event { at, jump, .. } => {
                    *at += offset as f64 / 1000.0;
                    if let Some(jump) = jump {
                        match &mut jump.to {
                            Destination::Time(at) => *at += offset as f64 / 1000.0,
                            Destination::Label(label) => {
                                *label = format!("encounter-{position}/{label}")
                            }
                        }
                    }
                    if let Some(slot) = &mut slot {
                        super::phase::put((*at * 1000.0).round() as i64, slot, "atMs")?;
                    }
                }
                Entry::Label { at, name } => {
                    *at += offset as f64 / 1000.0;
                    *name = format!("encounter-{position}/{name}");
                }
                Entry::AbilityCatalog { abilities, .. } => {
                    abilities.retain(|a| catalog.insert(a.id.clone()))
                }
                _ => {}
            }
            rows.push((Some(index), entry, slot));
        }
        // Reuse a scheduled opening, e.g. a second identical StartsUsing row makes R12S entry ambiguous.
        let scheduled_opening = rows.iter().position(|(_, _, slot)| {
            slot.as_ref().is_some_and(|slot| {
                slot["selector"] == true
                    && slot["abilityIds"] == json!([opening.ability_id])
                    && slot["source"] == opening.source
            })
        });
        if let Some(index) = scheduled_opening {
            // Keep the schedule's rounded clock, e.g. rounding a 1049.5ms median twice changes its grid point.
            opening.at_ms = rows
                .get(index)
                .and_then(|(_, _, slot)| slot.as_ref())
                .and_then(|slot| slot["atMs"].as_i64())
                .context("Missing scheduled checkpoint time")?
                - offset;
        }
        let at = (offset + opening.at_ms) as f64 / 1000.0;
        let selector = Entry::Event {
            at,
            name: "--sync--".into(),
            duration: None,
            jump: Some(Jump {
                to: Destination::Time(at),
                when: JumpWhen::Sync,
            }),
            sync: Some(Sync::Network(NetworkSync {
                log: LogType::StartsUsing,
                fields: BTreeMap::from([
                    (
                        "id".into(),
                        FieldPattern::One(format!("{:X}", opening.ability_id)),
                    ),
                    (
                        "source".into(),
                        FieldPattern::One(regress::escape(&opening.source)),
                    ),
                ]),
                enabled: true,
                window: Some([at, 2.5]),
            })),
            note: Some(format!(
                "Checkpoint encounter {}: select this range from either fight start or the preceding segment.",
                keys.get(position)
                    .context("Missing checkpoint key")?
                    .encounter
            )),
        };
        let insertion = rows
            .iter()
            .position(|(_, entry, _)| match entry {
                Entry::Event { at: time, .. } | Entry::Label { at: time, .. } => *time >= at,
                _ => false,
            })
            .unwrap_or(rows.len());
        let opening_row = if let Some(existing) = scheduled_opening {
            rows.get_mut(existing)
                .context("Missing scheduled checkpoint opening")?
                .1 = selector;
            existing
        } else {
            rows.insert(insertion,(None,selector,Some(json!({"selector":true,"predictive":false,"evidence":"encounterEntry","abilityIds":[opening.ability_id],"samples":samples,"atMs":offset+opening.at_ms}))));
            insertion
        };
        for (position, (local, entry, slot)) in rows.into_iter().enumerate() {
            if position == opening_row {
                opening.entry = entries.len();
            }
            if let Some(index) = local {
                *map.get_mut(index)
                    .context("Missing checkpoint map position")? = entries.len();
            }
            if let Some(mut slot) = slot {
                super::phase::put(slots.len(), &mut slot, "id")?;
                slots.push(slot);
            }
            entries.push(entry);
        }
        segments.push(Segment {
            group: keys
                .get(position)
                .context("Missing checkpoint group")?
                .clone(),
            offset_ms: offset,
            yaml,
            report,
            entry_map: map,
            opening,
        });
        let end = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Event { at, .. } | Entry::Label { at, .. } => {
                    Some((*at * 1000.0).round() as i64)
                }
                _ => None,
            })
            .max()
            .unwrap_or(0);
        offset = end
            .checked_add((lookahead * 1000.0).ceil() as i64)
            .and_then(|n| n.checked_add(1002600))
            .context("Checkpoint clock overflow")?
            / 1000000
            * 1000000;
    }
    let yaml = draft::serialize_draft(entries, mode.reset_on())?;
    let all_inputs: Vec<_> = segments
        .iter()
        .map(|s| inputs(&s.report))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let selected: Vec<_> = all_inputs
        .iter()
        .filter_map(|i| i.pointer("/input/file").cloned())
        .collect();
    let excluded: Vec<_> = segments
        .iter()
        .flat_map(|s| {
            s.report
                .pointer("/inputSelection/excluded")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned()
        })
        .collect();
    let report = json!({"status":"draft","mode":mode,"group":keys.first(),"groups":keys,"slots":slots,"segments":segments,
        "inputs":all_inputs,
        "outputCoverage":segments.iter().flat_map(|s|s.report.get("outputCoverage").and_then(Value::as_array).into_iter().flatten()).collect::<Vec<_>>(),
        "inputSelection":{"policy":"Select inputs independently in each observed checkpoint encounter.","selected":selected,"excluded":excluded},
        "validation":{"schemaAndSemantic":true,"replay":false,"cactbotParser":false,"runtime":false}});
    Ok((yaml, report))
}

pub(super) fn source_report<'a>(report: &'a Value, key: &GroupKey) -> Result<&'a Value> {
    if let Some(segments) = report.get("segments").and_then(Value::as_array) {
        let expected = serde_json::to_value(key)?;
        return segments
            .iter()
            .find(|s| s.get("group") == Some(&expected))
            .and_then(|s| s.get("report"))
            .context("Replay input is not in the combined timeline");
    }
    Ok(report)
}

pub(super) fn evidence(
    yaml: &str,
    report: &Value,
    key: &GroupKey,
    pull: &Pull,
    log: &CollectedLog,
    peers: &BTreeMap<&str, &Pull>,
) -> Result<crate::timeline::replay::Evidence> {
    let segments: Vec<Segment> = serde_json::from_value(
        report
            .get("segments")
            .context("Missing combined segments")?
            .clone(),
    )?;
    let segment = segments
        .iter()
        .find(|s| &s.group == key)
        .context("Missing replay checkpoint")?;
    let local = super::replay::evidence(&segment.yaml, &segment.report, key, pull, log, peers)?;
    let map = |index: usize| {
        segment
            .entry_map
            .get(index)
            .copied()
            .context("Missing checkpoint entry mapping")
    };
    let set = |indices: BTreeSet<usize>| {
        indices
            .into_iter()
            .map(map)
            .collect::<Result<BTreeSet<_>>>()
    };
    let mut evidence = crate::timeline::replay::Evidence {
        expected: local
            .expected
            .into_iter()
            .map(|(i, v)| Ok((map(i)?, v)))
            .collect::<Result<_>>()?,
        observed_packets: local
            .observed_packets
            .into_iter()
            .map(|(i, v)| Ok((map(i)?, v)))
            .collect::<Result<_>>()?,
        censored: set(local.censored)?,
        missing: set(local.missing)?,
        required_missing: set(local.required_missing)?,
        inactive: set(local.inactive)?,
        unordered: set(local.unordered)?,
        packet_wave_selectors: set(local.packet_wave_selectors)?,
        lookahead_ms: local.lookahead_ms,
    };
    let actors: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .map(|a| (a.id, a.name.as_str()))
        .collect();
    if let Some(row) = pull.occurrences.iter().find(|r| {
        r.kind == "begincast"
            && r.ability_id == segment.opening.ability_id
            && actors.get(&r.actor_id).copied() == Some(segment.opening.source.as_str())
    }) {
        evidence
            .expected
            .entry(segment.opening.entry)
            .or_default()
            .insert(row.event_index);
    } else if pull.end_ms < segment.opening.at_ms {
        evidence.censored.insert(segment.opening.entry);
    } else {
        evidence.missing.insert(segment.opening.entry);
    }
    let timeline: Timeline = serde_saphyr::from_str(yaml)?;
    let own: BTreeSet<_> = segment
        .entry_map
        .iter()
        .copied()
        .chain([segment.opening.entry])
        .collect();
    evidence.inactive.extend(
        timeline
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                matches!(e, Entry::Event { .. }) && !e.is_combat_start() && !own.contains(i)
            })
            .map(|(i, _)| i),
    );
    Ok(evidence)
}

pub(super) fn render(report: &Value) -> Result<String> {
    let segments: Vec<Segment> = serde_json::from_value(
        report
            .get("segments")
            .context("Missing combined segments")?
            .clone(),
    )?;
    let mut out = String::from(
        "# 체크포인트 통합 타임라인\n\n| encounter | 가상 시작 (초) | 구간 행 |\n| --- | ---: | ---: |\n",
    );
    for s in &segments {
        writeln!(
            out,
            "| {} | {:.1} | {} |",
            s.group.encounter,
            s.offset_ms as f64 / 1000.0,
            s.report
                .get("slots")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        )?;
    }
    for s in &segments {
        writeln!(
            out,
            "\n## Encounter {}\n\n{}",
            s.group.encounter,
            super::report::render(&s.report)?
        )?;
    }
    Ok(out)
}

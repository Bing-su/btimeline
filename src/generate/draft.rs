use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use itertools::Itertools;
use path_slash::PathBufExt as _;
use serde::Serialize;
use serde_json::Value;
use usage::ValueEnum;

use super::report::{
    BossSegment, CollapsedCast, ReportInput, SingleBlock, SingleReport, SingleSlot, TimeStatistics,
    Validation,
};
use super::{Occurrence, Source, input, load_one};
use crate::timeline::{Ability, Entry, FieldPattern, LogType, NetworkSync, Sync, Timeline};

const SCHEMA_HEADER: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/Bing-su/btimeline/main/schema/btimeline-v1.schema.json\n";
const SYNC_WINDOW_MS: i64 = 2500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SyncConflict {
    event_index: usize,
    conflicting_event_indices: Vec<usize>,
    reason: &'static str,
}

fn rounded_milliseconds(ms: i64) -> i64 {
    // Match the emitted tenth: 5,050 ms becomes 5,100 ms for sync-window checks too.
    (ms.div_euclid(100) + i64::from(ms.rem_euclid(100) >= 50)) * 100
}

pub(super) fn rounded_seconds(ms: i64) -> f64 {
    rounded_milliseconds(ms) as f64 / 1000.0
}

fn same_signal(
    event: &crate::fflogs::model::Event,
    row: &Occurrence,
    source: &str,
    names: &BTreeMap<i64, String>,
) -> bool {
    event.kind == "cast"
        && event.ability_game_id == Some(row.ability_id)
        && event
            .source_id
            .and_then(|id| names.get(&id))
            .is_some_and(|name| name == source)
        && event
            .timestamp
            .abs_diff(row.timestamp_ms - row.relative_ms + rounded_milliseconds(row.relative_ms))
            <= SYNC_WINDOW_MS as u64
        && (event.timestamp != row.timestamp_ms
            || event.source_id != Some(row.actor_id)
            || event.source_instance != row.instance)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum GenerateMode {
    Dungeon,
    Raid,
}

#[cfg(test)]
pub fn generate(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mode: GenerateMode,
) -> Result<()> {
    generate_selected(input, output, mode, None, None, None, 30.0)
}

pub fn generate_selected(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mode: GenerateMode,
    name: Option<&str>,
    encounter: Option<i64>,
    difficulty: Option<i64>,
    lookahead: f64,
) -> Result<()> {
    ensure!(
        lookahead.is_finite() && (0.0..=3600.0).contains(&lookahead),
        "Lookahead must be between 0 and 3600 seconds"
    );
    let input = input.as_ref();
    let output = output.as_ref();
    let report_path = output.with_extension("report.json");
    let markdown_path = output.with_extension("report.md");
    ensure!(
        !output.exists() && !report_path.exists() && !markdown_path.exists(),
        "Output already exists"
    );

    let group = input::select_group(input, name, encounter, difficulty)?;
    let (yaml, report) = if group.pulls.len() == 1 {
        let source = load_one(&PathBuf::from_slash(
            &group.pulls.first().context("Missing pull")?.file,
        ))?;
        let draft = build_single(&source, mode)?;
        (serialize_draft(draft.entries)?, draft.report)
    } else {
        super::multi::build(group, mode, lookahead)?
    };
    let report_bytes = format!("{}\n", serde_json::to_string_pretty(&report)?);
    let markdown = super::report::render(&report)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::output::write_new(&[
        (output, yaml.as_bytes()),
        (&report_path, report_bytes.as_bytes()),
        (&markdown_path, markdown.as_bytes()),
    ])
}

// Keep computed catalogs available to multi-pull generation without inspecting serialized entries.
#[derive(Debug)]
pub(super) struct SingleDraft {
    pub entries: Vec<Entry>,
    pub catalog: Vec<Ability>,
    pub report: Value,
}

// Validate each input before consensus can omit its rows, e.g. an invalid ability name still fails.
pub(super) fn build_single(source: &Source, mode: GenerateMode) -> Result<SingleDraft> {
    let pull = &source.pull;
    let data = &source.log;
    let names: BTreeMap<i64, String> = data
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.clone()))
        .collect();
    let abilities: BTreeMap<i64, String> = data
        .report
        .master_data
        .abilities
        .iter()
        .map(|ability| (ability.game_id, ability.name.clone()))
        .collect();
    let fight = data.report.fights.first().context("Missing fight")?;
    let enemies: BTreeSet<i64> = fight
        .enemy_npcs
        .iter()
        .chain(&fight.enemy_pets)
        .map(|enemy| enemy.id)
        .chain(fight.enemy_players.iter().flatten().copied())
        .collect();
    let boss_ids: BTreeSet<i64> = data
        .report
        .master_data
        .actors
        .iter()
        .filter(|actor| actor.sub_type == "Boss" && enemies.contains(&actor.id))
        .map(|actor| actor.id)
        .collect();
    let mut boss_spans = BTreeMap::<i64, (i64, i64)>::new();
    // Record boss boundaries for the report and the optional output filter.
    for event in &data.events {
        for id in [event.source_id, event.target_id].into_iter().flatten() {
            if boss_ids.contains(&id) {
                let span = boss_spans
                    .entry(id)
                    .or_insert((event.timestamp, event.timestamp));
                span.0 = span.0.min(event.timestamp);
                span.1 = span.1.max(event.timestamp);
            }
        }
    }
    if mode == GenerateMode::Dungeon {
        ensure!(!boss_spans.is_empty(), "No observed boss segment");
    }
    let in_boss_span = |at: i64| {
        mode == GenerateMode::Raid
            || boss_spans
                .values()
                .any(|&(start, end)| start <= at && at <= end)
    };
    let mut catalog = Vec::new();
    let mut catalog_ids = BTreeSet::new();
    // Preserve source order at equal timestamps so first-seen catalog entries stay deterministic.
    for event in data.events.iter().sorted_by_key(|event| event.timestamp) {
        if !in_boss_span(event.timestamp)
            || !matches!(event.kind.as_str(), "begincast" | "cast")
            || !event.source_id.is_some_and(|id| enemies.contains(&id))
        {
            continue;
        }
        let Some(id) = event.ability_game_id else {
            continue;
        };
        if catalog_ids.insert(id) {
            let name = abilities.get(&id).context("Missing catalog ability")?;
            catalog.push(Ability {
                id: format!("{id:X}"),
                name: name.clone(),
                note: None,
                ignored: false,
            });
        }
    }

    let mut entries = vec![Entry::Note {
        text: format!(
            "Draft from FFLogs report {} fight {} revision {} logVersion {}; times are relative to fight start.",
            pull.report, pull.fight, pull.revision, data.report.master_data.log_version
        ),
    }];
    let mut conflicts = Vec::new();
    let mut used = BTreeMap::new();
    let mut collapsed: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut emitted = Vec::new();
    let mut slots = Vec::new();
    for row in pull
        .occurrences
        .iter()
        .filter(|row| row.kind == "cast" && in_boss_span(row.timestamp_ms))
    {
        // One display row for simultaneous instances of one actor/ability, e.g. two helpers at 1000 ms.
        let key = (row.timestamp_ms, row.actor_id, row.ability_id);
        if let Some(&representative) = used.get(&key) {
            collapsed
                .entry(representative)
                .or_default()
                .push(row.event_index);
            continue;
        }
        used.insert(key, row.event_index);
        let source = names
            .get(&row.actor_id)
            .context("Missing cast source name")?;
        let name = abilities
            .get(&row.ability_id)
            .context("Missing cast ability name")?;
        let matching: Vec<usize> = data
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| same_signal(event, row, source, &names))
            .map(|(index, _)| index)
            .collect();
        let invalid_source = source.contains(['#', '"', '\r', '\n']);
        let reason = if invalid_source {
            Some("source name cannot be rendered safely as a sync")
        } else if !matching.is_empty() {
            Some("another cast matches within the default sync window")
        } else {
            None
        };
        let mut fields = BTreeMap::new();
        fields.insert(
            "id".into(),
            FieldPattern::One(format!("^{:X}$", row.ability_id)),
        );
        if !invalid_source {
            fields.insert(
                "source".into(),
                FieldPattern::One(format!("^{}$", regress::escape(source))),
            );
        }
        if let Some(reason) = reason {
            conflicts.push(SyncConflict {
                event_index: row.event_index,
                conflicting_event_indices: matching,
                reason,
            });
        }
        entries.push(Entry::Event {
            at: rounded_seconds(row.relative_ms),
            name: name.clone(),
            duration: None,
            sync: Some(Sync::Network(NetworkSync {
                log: LogType::Ability,
                fields,
                enabled: reason.is_none(),
                window: None,
            })),
            jump: None,
            note: reason
                .map(|reason| format!("Sync disabled: {reason}; source event {}", row.event_index)),
        });
        emitted.push(row.event_index);
        slots.push(SingleSlot {
            event_index: row.event_index,
            sample_count: 1,
            time_ms: row.relative_ms,
            block: 0,
            time: TimeStatistics {
                median_ms: row.relative_ms,
                min_ms: row.relative_ms,
                max_ms: row.relative_ms,
                sample_count: 1,
            },
            evidence: "observed",
        });
    }
    entries.push(Entry::AbilityCatalog {
        abilities: catalog.clone(),
        phase: None,
    });
    let timeline = Timeline {
        schema_version: 1,
        hide_names: Vec::new(),
        entries,
    };
    crate::timeline::validate_value(serde_json::to_value(&timeline)?)
        .context("Generated draft failed validation")?;
    let report = serde_json::to_value(SingleReport {
        status: "draft",
        mode,
        boss_segments: boss_spans
            .into_iter()
            .map(|(actor_id, (start, end))| BossSegment {
                actor_id,
                start_ms: start - fight.start_time,
                end_ms: end - fight.start_time,
            })
            .collect(),
        input: ReportInput {
            file: &pull.file,
            sha256: &source.sha256,
            report: &pull.report,
            fight: pull.fight,
            name: &pull.name,
            revision: pull.revision,
            game_version: pull.game_version,
            log_version: pull.log_version,
            complete: data.collection.complete,
        },
        group: &source.key,
        kill: pull.kill,
        end_ms: pull.end_ms,
        occurrences: &pull.occurrences,
        actor_names: names,
        ability_names: abilities,
        emitted_event_indices: emitted,
        collapsed_casts: collapsed
            .into_iter()
            .map(
                |(representative_event_index, omitted_event_indices)| CollapsedCast {
                    representative_event_index,
                    omitted_event_indices,
                },
            )
            .collect(),
        slots,
        blocks: vec![SingleBlock {
            id: 0,
            entry: "fightStart",
            time: TimeStatistics {
                median_ms: 0,
                min_ms: 0,
                max_ms: 0,
                sample_count: 1,
            },
        }],
        sync_conflicts: conflicts,
        validation: Validation::default(),
    })?;
    Ok(SingleDraft {
        entries: timeline.entries,
        catalog,
        report,
    })
}

pub(super) fn serialize_draft(entries: Vec<Entry>) -> Result<String> {
    let yaml = format!(
        "{SCHEMA_HEADER}{}",
        serde_saphyr::to_string(&Timeline {
            schema_version: 1,
            hide_names: Vec::new(),
            entries
        })?
    );
    crate::timeline::convert(&yaml).context("Generated draft failed validation")?;
    Ok(yaml)
}

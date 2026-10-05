use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use path_slash::PathBufExt as _;
use serde::Serialize;
use serde_json::{Value, json};
use usage::ValueEnum;

use super::{Occurrence, Source, load_one};

const SCHEMA_HEADER: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/Bing-su/btimeline/main/schema/btimeline-v1.schema.json\n";
const SYNC_WINDOW_MS: i64 = 2500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncConflict {
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
    generate_selected(input, output, mode, None, None, None)
}

pub fn generate_selected(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    mode: GenerateMode,
    name: Option<&str>,
    encounter: Option<i64>,
    difficulty: Option<i64>,
) -> Result<()> {
    let input = input.as_ref();
    let output = output.as_ref();
    let report_path = output.with_extension("report.json");
    let markdown_path = output.with_extension("report.md");
    ensure!(
        !output.exists() && !report_path.exists() && !markdown_path.exists(),
        "Output already exists"
    );

    let group = super::multi::select_group(input, name, encounter, difficulty)?;
    let (yaml, report) = if group.pulls.len() == 1 {
        let source = load_one(&PathBuf::from_slash(
            &group.pulls.first().context("Missing pull")?.file,
        ))?;
        let (entries, report) = build_single(&source, mode)?;
        (serialize_draft(entries)?, report)
    } else {
        super::multi::build(group, mode)?
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

// Return entries before serialization so multi-pull generation can reuse catalogs without a YAML round-trip.
pub(super) fn build_single(source: &Source, mode: GenerateMode) -> Result<(Vec<Value>, Value)> {
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
    let mut sorted_events: Vec<_> = data.events.iter().collect();
    sorted_events.sort_by_key(|event| event.timestamp);
    for event in sorted_events {
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
            catalog.push(json!({"id":format!("{id:X}"), "name":name}));
        }
    }

    let mut entries = vec![json!({"kind":"note", "text":format!(
        "Draft from FFLogs report {} fight {} revision {} logVersion {}; times are relative to fight start.",
        pull.report, pull.fight, pull.revision, data.report.master_data.log_version
    )})];
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
        let mut fields = serde_json::Map::new();
        fields.insert("id".into(), json!(format!("^{:X}$", row.ability_id)));
        if !invalid_source {
            fields.insert(
                "source".into(),
                json!(format!("^{}$", regress::escape(source))),
            );
        }
        let mut sync = json!({"log":"Ability", "fields":fields});
        if let Some(reason) = reason {
            sync.as_object_mut()
                .context("Missing sync object")?
                .insert("enabled".into(), json!(false));
            conflicts.push(SyncConflict {
                event_index: row.event_index,
                conflicting_event_indices: matching,
                reason,
            });
        }
        let mut event = json!({"kind":"event", "at":rounded_seconds(row.relative_ms),
            "name":name, "sync":sync});
        if let Some(reason) = reason {
            event
                .as_object_mut()
                .context("Missing event object")?
                .insert(
                    "note".into(),
                    json!(format!(
                        "Sync disabled: {reason}; source event {}",
                        row.event_index
                    )),
                );
        }
        entries.push(event);
        emitted.push(row.event_index);
        slots.push(json!({"eventIndex":row.event_index, "sampleCount":1,
            "timeMs":row.relative_ms, "block":0,
            "time":{"medianMs":row.relative_ms, "minMs":row.relative_ms, "maxMs":row.relative_ms, "sampleCount":1},
            "evidence":"observed"}));
    }
    entries.push(json!({"kind":"abilityCatalog", "abilities":catalog}));
    crate::timeline::validate_value(json!({"schemaVersion":1, "entries":entries}))
        .context("Generated draft failed validation")?;
    let report: Value = json!({
        "status":"draft",
        "mode":mode,
        "bossSegments":boss_spans.into_iter().map(|(actor_id, (start, end))| json!({
            "actorId":actor_id, "startMs":start - fight.start_time, "endMs":end - fight.start_time
        })).collect::<Vec<_>>(),
        "input":{"file":pull.file, "report":pull.report, "fight":pull.fight,
            "name":pull.name,
            "revision":pull.revision, "gameVersion":pull.game_version, "logVersion":pull.log_version,
            "complete":data.collection.complete},
        "group":source.key,
        "kill":pull.kill,
        "endMs":pull.end_ms,
        "occurrences":pull.occurrences,
        "actorNames":names,
        "abilityNames":abilities,
        "emittedEventIndices":emitted,
        "collapsedCasts":collapsed.into_iter().map(|(representative, omitted)| json!({
            "representativeEventIndex":representative, "omittedEventIndices":omitted
        })).collect::<Vec<_>>(),
        "slots":slots,
        "blocks":[{"id":0,"entry":"fightStart","time":{"medianMs":0,"minMs":0,"maxMs":0,"sampleCount":1}}],
        "syncConflicts":conflicts,
        "validation":{"schemaAndSemantic":true, "replay":false, "cactbotParser":false, "runtime":false}
    });
    Ok((entries, report))
}

pub(super) fn serialize_draft(entries: Vec<Value>) -> Result<String> {
    let yaml = format!(
        "{SCHEMA_HEADER}{}",
        serde_saphyr::to_string(&json!({
            "schemaVersion":1, "entries":entries
        }))?
    );
    crate::timeline::convert(&yaml).context("Generated draft failed validation")?;
    Ok(yaml)
}

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::replay::{Observation, Recording};
use crate::fflogs::model::{CollectedLog, Event};

fn text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn fields(event: &Event, names: &BTreeMap<i64, &str>) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    for (name, id) in [("source", event.source_id), ("target", event.target_id)] {
        if let Some(actor) = id.and_then(|id| names.get(&id)) {
            fields.insert(name.into(), (*actor).into());
        }
    }
    for (name, value) in [
        ("sourceID", event.source_id),
        ("targetID", event.target_id),
        ("sourceInstance", event.source_instance),
        ("abilityGameID", event.ability_game_id),
        ("fight", event.fight),
    ] {
        if let Some(value) = value {
            fields.insert(format!("raw.{name}"), value.to_string());
        }
    }
    if let Some(melee) = event.melee {
        fields.insert("raw.melee".into(), melee.to_string());
    }
    if let Some(id) = event.ability_game_id.filter(|id| *id >= 0) {
        fields.insert("id".into(), format!("{id:X}"));
    }
    fields.extend(
        event
            .extra
            .iter()
            .map(|(key, value)| (format!("raw.{key}"), text(value))),
    );
    fields
}

// Keep FFLogs events in their own namespace: an ACT signal is not inferred from a similar name.
pub(crate) fn from_fflogs(log: &CollectedLog) -> Result<Recording> {
    let fight = log.report.fights.first().context("Missing fight")?;
    ensure!(
        fight.start_time >= 0 && fight.end_time >= fight.start_time,
        "Invalid fight time range"
    );
    let names: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.as_str()))
        .collect();
    let mut observations = Vec::with_capacity(log.events.len());
    for (index, event) in log.events.iter().enumerate() {
        ensure!(
            !event.kind.is_empty(),
            "FFLogs event {index} has an empty type"
        );
        ensure!(
            (fight.start_time..=fight.end_time).contains(&event.timestamp),
            "FFLogs event {index} is outside the fight"
        );
        observations.push(Observation {
            at_ms: event.timestamp - fight.start_time,
            kind: format!("FFLogs.{}", event.kind),
            fields: fields(event, &names),
            source_event_index: Some(index),
        });
    }
    // FFLogs can return late packets; equal timestamps retain their original source order.
    observations.sort_by_key(|observation| observation.at_ms);
    Ok(Recording {
        version: 1,
        end_ms: fight.end_time - fight.start_time,
        observations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reordered_fixture() -> Result<Recording> {
        let mut log: CollectedLog =
            serde_json::from_str(include_str!("../../tests/fixtures/pull.json"))?;
        log.events.swap(0, 1);
        log.events.get_mut(2).context("third event")?.timestamp = 2000;
        log.events
            .get_mut(1)
            .context("second event")?
            .extra
            .insert("nested".into(), serde_json::json!({"value": 7}));
        from_fflogs(&log)
    }

    #[test]
    fn normalization_preserves_source_indices_and_equal_timestamp_order() {
        let recording = reordered_fixture().unwrap();
        let indices: Vec<_> = recording
            .observations
            .iter()
            .filter_map(|event| event.source_event_index)
            .collect();
        assert_eq!(indices, [1, 2, 0]);
        let first = recording.observations.first().unwrap();
        assert_eq!(first.kind, "FFLogs.cast");
        assert_eq!(first.at_ms, 1000);
        assert_eq!(
            first.fields.get("source").map(String::as_str),
            Some("Synthetic Boss")
        );
        assert_eq!(first.fields.get("id").map(String::as_str), Some("B528"));
        assert_eq!(
            first.fields.get("raw.nested").map(String::as_str),
            Some(r#"{"value":7}"#)
        );
    }
}

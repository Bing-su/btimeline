use std::collections::BTreeMap;

use garde::Validate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// Keep fields outside the timeline contract intact when collected logs are saved again.
#[derive(Debug, Deserialize, Serialize, Validate)]
#[garde(allow_unvalidated)]
#[garde(custom(validate_collected_log))]
pub struct CollectedLog {
    pub report: Report,
    pub events: Vec<Event>,
    #[garde(dive)]
    pub collection: Collection,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub code: String,
    pub revision: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub fights: Vec<Fight>,
    pub master_data: MasterData,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fight {
    pub id: i64,
    pub name: String,
    #[serde(rename = "encounterID")]
    pub encounter_id: i64,
    pub difficulty: i64,
    pub start_time: i64,
    pub end_time: i64,
    pub in_progress: bool,
    pub kill: bool,
    #[serde(rename = "enemyNPCs")]
    pub enemy_npcs: Vec<Enemy>,
    #[serde(rename = "enemyPets")]
    pub enemy_pets: Vec<Enemy>,
    #[serde(rename = "enemyPlayers", skip_serializing_if = "Option::is_none")]
    pub enemy_players: Option<Vec<i64>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Enemy {
    pub id: i64,
    #[serde(rename = "gameID")]
    pub game_id: i64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterData {
    pub lang: String,
    pub game_version: i64,
    pub log_version: i64,
    pub actors: Vec<Actor>,
    pub abilities: Vec<Ability>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Actor {
    pub id: i64,
    pub name: String,
    #[serde(rename = "gameID")]
    pub game_id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "subType")]
    pub sub_type: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Ability {
    pub name: String,
    #[serde(rename = "gameID")]
    pub game_id: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Event {
    pub timestamp: i64,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fight: Option<i64>,
    #[serde(rename = "sourceID", skip_serializing_if = "Option::is_none")]
    pub source_id: Option<i64>,
    #[serde(rename = "targetID", skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i64>,
    #[serde(rename = "sourceInstance", skip_serializing_if = "Option::is_none")]
    pub source_instance: Option<i64>,
    #[serde(rename = "abilityGameID", skip_serializing_if = "Option::is_none")]
    pub ability_game_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub melee: Option<bool>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize, Validate)]
#[garde(allow_unvalidated)]
#[garde(custom(validate_collection))]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    #[garde(range(equal = 1))]
    pub schema_version: i64,
    pub tool_version: String,
    pub collected_at_unix_ms: u64,
    pub report_code: String,
    #[serde(rename = "fightID")]
    pub fight_id: i64,
    #[garde(range(min = 1))]
    pub page_count: usize,
    #[garde(length(min = 1), inner(range(min = 0.0, max = self.end_time)))]
    pub page_start_times: Vec<f64>,
    pub event_count: usize,
    pub complete: bool,
    pub next_page_timestamp: Option<f64>,
    #[garde(range(min = 0.0, max = self.end_time))]
    pub start_time: f64,
    #[garde(range(min = 0.0))]
    pub end_time: f64,
    pub requests: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

fn valid_millisecond(value: f64) -> bool {
    value.is_finite() && (0.0..9_223_372_036_854_775_808.0).contains(&value) && value.fract() == 0.0
}

fn check(valid: bool, message: &str) -> garde::Result {
    if valid {
        Ok(())
    } else {
        Err(garde::Error::new(message))
    }
}

// Validate the collector's own completion and pagination evidence before using its events.
fn validate_collection(collection: &Collection, _: &()) -> garde::Result {
    check(collection.complete, "Incomplete collection")?;
    check(
        collection.next_page_timestamp.is_none(),
        "Unfinished collection cursor",
    )?;
    check(
        valid_millisecond(collection.start_time) && valid_millisecond(collection.end_time),
        "Invalid collection time precision",
    )?;
    check(
        collection.page_count == collection.page_start_times.len(),
        "Invalid collection pages",
    )?;
    check(
        collection.page_start_times.first().copied() == Some(collection.start_time)
            && collection
                .page_start_times
                .iter()
                .all(|&at| valid_millisecond(at))
            && collection
                .page_start_times
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
        "Invalid collection page cursor",
    )
}

// Cross-check the manifest against the report and event array, not just itself.
fn validate_collected_log(data: &CollectedLog, _: &()) -> garde::Result {
    let [fight] = data.report.fights.as_slice() else {
        return Err(garde::Error::new("Expected exactly one fight"));
    };
    check(
        data.collection.report_code == data.report.code && data.collection.fight_id == fight.id,
        "Collection identity mismatch",
    )?;
    check(
        data.collection.start_time == fight.start_time as f64
            && data.collection.end_time == fight.end_time as f64,
        "Collection time range mismatch",
    )?;
    check(
        data.collection.event_count == data.events.len(),
        "Collection event count mismatch",
    )
}

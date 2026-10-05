mod pairing;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use garde::Validate;
use path_slash::PathExt as _;
use serde::Serialize;

use crate::fflogs::model::CollectedLog;

use pairing::{PendingStart, start_for};

// Align equivalent FFLogs segments; an encounter ID can identify one checkpoint segment of a battle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupKey {
    encounter: i64,
    difficulty: i64,
}

#[derive(Debug, Serialize)]
pub struct Occurrence {
    pub event_index: usize,
    pub timestamp_ms: i64,
    pub relative_ms: i64,
    pub simultaneous: usize,
    pub kind: String,
    pub actor_id: i64,
    pub actor_game_id: i64,
    pub role: String,
    pub instance: Option<i64>,
    pub ability_id: i64,
    pub start_event_index: Option<usize>,
    pub start_timestamp_ms: Option<i64>,
    pub completion_event_index: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Pull {
    pub file: String,
    pub report: String,
    pub revision: i64,
    pub game_version: i64,
    pub log_version: i64,
    pub fight: i64,
    pub name: String,
    pub kill: bool,
    pub end_ms: i64,
    pub occurrences: Vec<Occurrence>,
    #[serde(skip)]
    actors: BTreeMap<i64, (i64, String)>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub key: GroupKey,
    pub pulls: Vec<Pull>,
}

// Keep normalized and raw events from one read, e.g. pairing and sync checks share source indices.
struct Source {
    key: GroupKey,
    pull: Pull,
    log: CollectedLog,
}

fn slash_path(path: &Path) -> String {
    let path = path.to_slash_lossy();
    // path-slash retains Windows prefixes; normalize e.g. \\server\share too, preserving Unix filenames.
    #[cfg(windows)]
    {
        path.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        path.into_owned()
    }
}

fn load_one(path: &Path) -> Result<Source> {
    let data: CollectedLog = serde_json::from_slice(&fs::read(path)?)?;
    data.validate().context("Invalid collection")?;
    let report = &data.report;
    let events = &data.events;
    let fight = report
        .fights
        .first()
        .context("Expected exactly one fight")?;
    let master = &report.master_data;
    let start = fight.start_time;
    let end = fight.end_time;
    ensure!(start >= 0 && end >= start, "Invalid fight time range");
    ensure!(!fight.in_progress, "Fight still in progress");
    let key = GroupKey {
        encounter: fight.encounter_id,
        difficulty: fight.difficulty,
    };
    ensure!(key.encounter > 0, "Invalid encounter ID");
    let mut actors = BTreeMap::new();
    for actor in &master.actors {
        let value = (actor.game_id, format!("{}/{}", actor.kind, actor.sub_type));
        ensure!(
            actors.insert(actor.id, value).is_none(),
            "Duplicate actor ID"
        );
    }
    let mut abilities = BTreeSet::new();
    for ability in &master.abilities {
        ensure!(abilities.insert(ability.game_id), "Duplicate ability ID");
    }
    let mut enemies = BTreeSet::new();
    for enemy in fight.enemy_npcs.iter().chain(&fight.enemy_pets) {
        ensure!(
            actors
                .get(&enemy.id)
                .is_some_and(|actor| actor.0 == enemy.game_id),
            "Enemy actor reference mismatch"
        );
        enemies.insert(enemy.id);
    }
    for &id in fight.enemy_players.iter().flatten() {
        ensure!(actors.contains_key(&id), "Enemy player reference mismatch");
        enemies.insert(id);
    }
    let mut indexed = Vec::new();
    for (index, event) in events.iter().enumerate() {
        let at = event.timestamp;
        ensure!(
            at >= start && at <= end,
            "Event {index} outside fight range"
        );
        if let Some(fight_ref) = event.fight {
            ensure!(
                fight_ref == fight.id,
                "Event {index} fight reference mismatch"
            );
        }
        for (name, reference) in [("sourceID", event.source_id), ("targetID", event.target_id)] {
            if let Some(id) = reference {
                ensure!(actors.contains_key(&id), "Event {index} invalid {name}");
            }
        }
        if !matches!(event.kind.as_str(), "begincast" | "cast")
            || (event.kind == "cast" && event.melee == Some(true))
        {
            continue;
        }
        let actor = event.source_id.context("Cast missing sourceID")?;
        if !enemies.contains(&actor) {
            continue;
        }
        let ability = event
            .ability_game_id
            .context("Enemy cast missing abilityGameID")?;
        ensure!(
            abilities.contains(&ability),
            "Event {index} missing ability {ability}"
        );
        indexed.push((at, index, actor, ability));
    }
    actors.retain(|id, _| enemies.contains(id));
    indexed.sort_by_key(|&(at, _, _, _)| at); // Stable: source order survives equal timestamps.
    let mut starts: Vec<PendingStart> = Vec::new();
    let mut occurrences: Vec<Occurrence> = Vec::with_capacity(indexed.len());
    let mut simultaneous = 0;
    let mut last_at = None;
    for (at, index, actor, ability) in indexed {
        if last_at != Some(at) {
            simultaneous += 1;
            last_at = Some(at);
        }
        let event = events.get(index).context("Missing indexed event")?;
        let paired = if event.kind == "begincast" {
            starts.retain(|pending| {
                pending.actor != actor
                    || pending.instance != event.source_instance
                    || pending.ability != ability
            });
            starts.push(PendingStart {
                actor,
                instance: event.source_instance,
                ability,
                event_index: index,
                timestamp: at,
                row: occurrences.len(),
            });
            None
        } else {
            start_for(&mut starts, actor, event.source_instance, ability, at)
        };
        if let Some(ref start) = paired {
            occurrences
                .get_mut(start.row)
                .context("Missing cast start row")?
                .completion_event_index = Some(index);
        }
        let identity = actors.get(&actor).context("Missing enemy actor")?;
        occurrences.push(Occurrence {
            event_index: index,
            timestamp_ms: at,
            relative_ms: at - start,
            simultaneous,
            kind: event.kind.clone(),
            actor_id: actor,
            actor_game_id: identity.0,
            role: identity.1.clone(),
            instance: event.source_instance,
            ability_id: ability,
            start_event_index: paired.as_ref().map(|start| start.event_index),
            start_timestamp_ms: paired.as_ref().map(|start| start.timestamp),
            completion_event_index: None,
        });
    }
    Ok(Source {
        key,
        pull: Pull {
            file: slash_path(path),
            report: report.code.clone(),
            revision: report.revision,
            game_version: master.game_version,
            log_version: master.log_version,
            fight: fight.id,
            name: fight.name.clone(),
            kill: fight.kill,
            end_ms: end - start,
            occurrences,
            actors,
        },
        log: data,
    })
}

pub fn inspect(paths: &[impl AsRef<Path>]) -> Result<Vec<Group>> {
    ensure!(!paths.is_empty(), "Supply at least one input file");
    let mut groups: BTreeMap<GroupKey, Vec<Pull>> = BTreeMap::new();
    let mut identities = BTreeSet::new();
    for path in paths {
        let path = path.as_ref();
        let Source { key, pull, .. } =
            load_one(path).with_context(|| format!("Invalid input {}", path.display()))?;
        ensure!(
            identities.insert((pull.report.clone(), pull.fight)),
            "Duplicate pull input"
        );
        // Versions describe provenance, not encounter identity: parser 74 and 76 may contain the same casts.
        // Compare normalized actor identities below; P4/P5 retain any observed sequence differences.
        let peers = groups.entry(key).or_default();
        for peer in peers.iter() {
            for actor in pull.actors.values() {
                ensure!(
                    !peer
                        .actors
                        .values()
                        .any(|old| old.0 == actor.0 && old.1 != actor.1),
                    "Actor role conflict for game ID {}",
                    actor.0
                );
            }
        }
        peers.push(pull);
    }
    Ok(groups
        .into_iter()
        .map(|(key, pulls)| Group { key, pulls })
        .collect())
}

mod alignment;
mod draft;
mod multi;
mod report;
pub use alignment::align;
#[cfg(test)]
use draft::generate;
pub use draft::{GenerateMode, generate_selected};
pub use report::markdown_file;

#[cfg(test)]
mod tests;

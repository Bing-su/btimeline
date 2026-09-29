use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use garde::Validate;
use serde::Serialize;

use crate::fflogs::model::CollectedLog;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupKey {
    encounter: i64,
    difficulty: i64,
    game_version: i64,
    log_version: i64,
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

struct PendingStart {
    actor: i64,
    instance: Option<i64>,
    ability: i64,
    event_index: usize,
    timestamp: i64,
    row: usize,
}

// A completion uses the latest start; a later start supersedes a canceled cast.
fn start_for(
    starts: &mut Vec<PendingStart>,
    actor: i64,
    instance: Option<i64>,
    ability: i64,
    at: i64,
) -> Option<PendingStart> {
    let position = starts.iter().rposition(|start| {
        start.actor == actor
            && start.instance == instance
            && start.ability == ability
            && start.timestamp <= at
    })?;
    Some(starts.remove(position))
}

fn load_one(path: &Path) -> Result<(GroupKey, Pull)> {
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
        game_version: master.game_version,
        log_version: master.log_version,
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
        let event = &events[index];
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
            occurrences[start.row].completion_event_index = Some(index);
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
    Ok((
        key,
        Pull {
            file: path.display().to_string(),
            report: report.code.clone(),
            revision: report.revision,
            fight: fight.id,
            name: fight.name.clone(),
            kill: fight.kill,
            end_ms: end - start,
            occurrences,
            actors,
        },
    ))
}

pub fn inspect(paths: &[impl AsRef<Path>]) -> Result<Vec<Group>> {
    ensure!(!paths.is_empty(), "Supply at least one input file");
    let mut groups: BTreeMap<GroupKey, Vec<Pull>> = BTreeMap::new();
    let mut identities = BTreeSet::new();
    for path in paths {
        let path = path.as_ref();
        let (key, pull) =
            load_one(path).with_context(|| format!("Invalid input {}", path.display()))?;
        ensure!(
            identities.insert((pull.report.clone(), pull.fight)),
            "Duplicate pull input"
        );
        // Version conflicts for the same encounter need review before any group is generated.
        ensure!(
            !groups.keys().any(|other| other.encounter == key.encounter
                && other.difficulty == key.difficulty
                && (other.game_version != key.game_version
                    || other.log_version != key.log_version)),
            "Version conflict for encounter {} difficulty {}",
            key.encounter,
            key.difficulty
        );
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

#[cfg(test)]
mod tests;

#[cfg(kani)]
mod proofs {
    use super::*;

    #[kani::proof]
    fn completion_never_follows_end() {
        let start: i64 = kani::any();
        let end: i64 = kani::any();
        kani::assume(start >= 0 && start <= 1000 && end >= 0 && end <= 1000);
        let mut starts = vec![PendingStart {
            actor: 1,
            instance: Some(2),
            ability: 3,
            event_index: 7,
            timestamp: start,
            row: 0,
        }];
        let matched = start_for(&mut starts, 1, Some(2), 3, end);
        assert_eq!(matched.is_some(), start <= end);
        assert_eq!(starts.len(), usize::from(start > end));
    }
}

#[cfg(kani)]
#[kani::proof]
fn completion_uses_latest_matching_start() {
    let older: i64 = kani::any();
    let newer: i64 = kani::any();
    let completed: i64 = kani::any();
    kani::assume(0 <= older && older < newer && newer <= completed && completed <= 1000);
    let mut starts = vec![
        PendingStart {
            actor: 1,
            instance: Some(2),
            ability: 3,
            event_index: 7,
            timestamp: older,
            row: 0,
        },
        PendingStart {
            actor: 1,
            instance: Some(2),
            ability: 3,
            event_index: 8,
            timestamp: newer,
            row: 1,
        },
    ];
    assert_eq!(
        start_for(&mut starts, 1, Some(2), 3, completed).map(|start| start.event_index),
        Some(8)
    );
}

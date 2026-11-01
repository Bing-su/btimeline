//! Validate collected logs and select compatible pulls before generation or replay.
//! Keep source event indices intact so reports can trace filtered rows back to raw evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use garde::Validate;
use indicatif::{ParallelProgressIterator, ProgressStyle};
use rayon::prelude::*;

use super::pairing::{PendingStart, start_for};
use super::{Group, GroupKey, Occurrence, Pull, Source, slash_path};
use crate::fflogs::model::CollectedLog;

// Validate optional phase boundaries even when inspection only exports casts.
fn validate_transitions(fight: &crate::fflogs::model::Fight) -> Result<()> {
    let start = fight.start_time;
    let end = fight.end_time;
    // Bound repeated mechanics to their observed phase, e.g. P2's first Past's End cannot match its second round.
    let mut transitions = Vec::new();
    if let Some(value) = fight
        .extra
        .get("phaseTransitions")
        .filter(|value| !value.is_null())
    {
        for phase in value.as_array().context("Invalid phaseTransitions")? {
            let id = phase["id"].as_i64().context("Invalid phase ID")?;
            let at = phase["startTime"]
                .as_i64()
                .context("Invalid phase startTime")?;
            ensure!(
                start <= at
                    && at <= end
                    && transitions
                        .last()
                        .is_none_or(|&(_, previous)| previous < at),
                "Invalid phase transition time range"
            );
            transitions.push((id, at));
        }
    }
    Ok(())
}

// Validate all raw references before filtering, e.g. friendly events remain checked.
fn indexed_casts(
    events: &[crate::fflogs::model::Event],
    fight: &crate::fflogs::model::Fight,
    actors: &BTreeMap<i64, (i64, String)>,
    enemies: &BTreeSet<i64>,
    abilities: &BTreeSet<i64>,
) -> Result<Vec<(i64, usize, i64, i64)>> {
    let start = fight.start_time;
    let end = fight.end_time;
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
    Ok(indexed)
}

// Pair casts without losing source indices or equal-time source order.
fn paired_occurrences(
    events: &[crate::fflogs::model::Event],
    mut indexed: Vec<(i64, usize, i64, i64)>,
    actors: &BTreeMap<i64, (i64, String)>,
    start: i64,
) -> Result<Vec<Occurrence>> {
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
            // A repeated start supersedes a cancelled cast; A-start,A-start,A-cast pairs only the second.
            // Leave the earlier occurrence unfinished so termination evidence remains visible.
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
    Ok(occurrences)
}

fn parse_collected(bytes: &[u8]) -> Result<CollectedLog> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let data: CollectedLog = serde_path_to_error::deserialize(&mut deserializer)?;
    // Consume the whole file, e.g. reject a second JSON value after an otherwise valid log.
    deserializer.end()?;
    data.validate().context("Invalid collection")?;
    Ok(data)
}

fn source_from_collected(path: &Path, data: &CollectedLog) -> Result<Source> {
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
    validate_transitions(fight)?;

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

    let indexed = indexed_casts(events, fight, &actors, &enemies, &abilities)?;
    actors.retain(|id, _| enemies.contains(id));
    let occurrences = paired_occurrences(events, indexed, &actors, start)?;
    // Include melee-only bosses in combat ownership, e.g. damage proves survival without a cast.

    // Retain actual defeat evidence, e.g. HP reaches zero before the arena opens or death is logged.

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
            occurrences: occurrences.into(),
            actors,
            // FFLogs virtual IDs include the report-local actor ID, e.g. 2000031 versus 2001111.
            virtual_actors: master
                .actors
                .iter()
                .filter(|actor| actor.id.checked_add(2_000_000) == Some(actor.game_id))
                .map(|actor| (actor.id, actor.name.clone()))
                .collect(),
        },
    })
}

fn load_one(path: &Path) -> Result<Source> {
    let bytes = fs::read(path)?;
    let data = parse_collected(&bytes)?;
    drop(bytes);
    source_from_collected(path, &data)
}

pub(crate) fn validated_collected(path: &Path, bytes: &[u8]) -> Result<CollectedLog> {
    let data = parse_collected(bytes)?;
    source_from_collected(path, &data)?;
    Ok(data)
}

pub fn inspect(paths: &[impl AsRef<Path>]) -> Result<Vec<Group>> {
    ensure!(!paths.is_empty(), "Supply at least one input file");
    let mut groups: BTreeMap<GroupKey, Vec<Pull>> = BTreeMap::new();
    let mut identities = BTreeSet::new();
    let paths: Vec<_> = paths.iter().map(AsRef::as_ref).collect();
    // Drop raw events in each worker; merge in input order so duplicates and errors stay deterministic.
    let loaded: Vec<_> = paths
        .par_iter()
        .map(|path| {
            let Source { key, pull, .. } =
                load_one(path).with_context(|| format!("Invalid input {}", path.display()))?;
            Ok::<_, anyhow::Error>((key, pull))
        })
        // Draw on stderr only in terminals, e.g. piped inspect-inputs JSON stays machine-readable.
        .progress_with_style(ProgressStyle::with_template(
            "Reading logs {wide_bar} {pos}/{len}",
        )?)
        .collect();
    for source in loaded {
        let (key, pull) = source?;
        ensure!(
            identities.insert((pull.report.clone(), pull.fight)),
            "Duplicate pull input"
        );
        // Versions describe provenance, not encounter identity: parser 74 and 76 may contain the same casts.
        // Compare normalized actor identities below; P4/P5 retain any observed sequence differences.
        let peers = groups.entry(key).or_default();
        for peer in peers.iter() {
            for (_, actor) in pull
                .actors
                .iter()
                .filter(|(id, _)| !pull.virtual_actors.contains_key(id))
            {
                // Virtual IDs are report-local, e.g. 2000036 can mean unrelated boss and helper actors.
                ensure!(
                    !peer.actors.iter().any(|(old_id, old)| !peer
                        .virtual_actors
                        .contains_key(old_id)
                        && old.0 == actor.0
                        && old.1 != actor.1),
                    "Actor role conflict for game ID {}",
                    actor.0
                );
            }
        }
        peers.push(pull);
    }
    let groups = groups
        .into_iter()
        .map(|(key, pulls)| Group { key, pulls })
        .collect();
    Ok(groups)
}

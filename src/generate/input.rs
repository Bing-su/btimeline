//! Validate collected logs and select compatible pulls before generation or replay.
//! Keep source event indices intact so reports can trace filtered rows back to raw evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use garde::Validate;
use itertools::Itertools;
use rayon::prelude::*;
use serde_json::Value;

use super::pairing::{PendingStart, start_for};
use super::{Group, GroupKey, Occurrence, Pull, Source, sha256, slash_path};
use crate::fflogs::model::CollectedLog;

pub(super) fn load_one(path: &Path) -> Result<Source> {
    let bytes = fs::read(path)?;
    let sha256 = sha256(&bytes);
    let data: CollectedLog = serde_json::from_slice(&bytes)?;
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
        sha256,
    })
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
                    .sorted()
                    .dedup()
                    .join(" / "),
                group.key.encounter,
                group.key.difficulty,
                group.pulls.len()
            )
        })
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

// Filter only normalized correspondence; raw casts still expose accidental sync activation.
// For example, a helper before the first boss interaction stays in collision/replay evidence.
pub(super) fn filter_boss_spans(pull: &mut Pull, report: &Value) -> Result<()> {
    let spans = report["bossSegments"]
        .as_array()
        .context("Missing boss segments")?;
    pull.occurrences.retain(|row| {
        spans.iter().any(|span| {
            span["startMs"]
                .as_i64()
                .is_some_and(|at| row.relative_ms >= at)
                && span["endMs"]
                    .as_i64()
                    .is_some_and(|at| row.relative_ms <= at)
        })
    });
    Ok(())
}

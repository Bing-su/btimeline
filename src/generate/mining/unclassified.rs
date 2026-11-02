use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

use super::{Identifier, Signal};
use crate::fflogs::model::CollectedLog;

// Enemy lists omit untargetable helpers; retain unowned NPC casts without declaring them hostile.
pub(super) fn casts(log: &CollectedLog, known: &BTreeSet<usize>) -> Result<Vec<(usize, Signal)>> {
    let actors: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .filter(|actor| {
            actor.id > 0
                && actor.kind == "NPC"
                && actor
                    .extra
                    .get("petOwner")
                    .is_none_or(serde_json::Value::is_null)
        })
        .map(|actor| (actor.id, actor))
        .collect();
    let abilities: BTreeSet<_> = log
        .report
        .master_data
        .abilities
        .iter()
        .map(|ability| ability.game_id)
        .collect();
    log.events
        .iter()
        .enumerate()
        .filter(|(index, event)| {
            !known.contains(index)
                && matches!(event.kind.as_str(), "begincast" | "cast")
                && !(event.kind == "cast" && event.melee == Some(true))
                && event
                    .source_id
                    .is_some_and(|actor| actors.contains_key(&actor))
        })
        .map(|(index, event)| {
            let actor = event
                .source_id
                .and_then(|actor| actors.get(&actor))
                .context("Missing NPC cast source")?;
            let ability = event
                .ability_game_id
                .context("Unclassified NPC cast missing abilityGameID")?;
            ensure!(
                abilities.contains(&ability),
                "Event {index} missing unclassified NPC ability {ability}"
            );
            Ok((
                index,
                Signal {
                    kind: event.kind.clone(),
                    identifier: Identifier::Ability(ability),
                    // Anonymous NPCs remain observations; never fabricate a runtime source name.
                    source: (!actor.name.trim().is_empty()).then(|| actor.name.clone()),
                    target: None,
                },
            ))
        })
        .collect()
}

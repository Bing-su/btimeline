use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};

use super::{Identifier, Signal};
use crate::fflogs::model::{CollectedLog, Event};

// Keep FFLogs identifier domains separate, e.g. a marker's abilityGameID may be zero.
fn identifier(event: &Event) -> Option<Identifier> {
    match event.kind.as_str() {
        "headmarker" => event
            .extra
            .get("markerID")?
            .as_i64()
            .filter(|id| *id >= 0)
            .map(Identifier::Marker),
        "tether" => event
            .extra
            .get("tetherID")?
            .as_i64()
            .filter(|id| *id >= 0)
            .map(Identifier::Tether),
        "applybuff" | "removebuff" | "refreshbuff" | "applydebuff" | "removedebuff"
        | "refreshdebuff" => event
            .ability_game_id
            .filter(|id| *id >= 0)
            .map(Identifier::Ability),
        _ => None,
    }
}

pub(super) fn select(log: &CollectedLog) -> Result<Vec<(usize, Signal)>> {
    let fight = log.report.fights.first().context("Missing fight")?;
    let enemies: BTreeSet<_> = fight
        .enemy_npcs
        .iter()
        .chain(&fight.enemy_pets)
        .map(|actor| actor.id)
        .chain(fight.enemy_players.iter().flatten().copied())
        .collect();
    let names: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.as_str()))
        .collect();
    let hostile_name = |id: Option<i64>| -> Result<Option<String>> {
        let Some(id) = id.filter(|id| enemies.contains(id)) else {
            return Ok(None);
        };
        let name = names.get(&id).context("Missing candidate actor name")?;
        // Missing endpoint names stay absent, e.g. a tether from an anonymous enemy to the boss.
        Ok((!name.trim().is_empty()).then(|| (*name).to_owned()))
    };
    let mut selected = Vec::new();
    for (index, event) in log.events.iter().enumerate() {
        let Some(identifier) = identifier(event) else {
            continue;
        };
        let source = hostile_name(event.source_id)?;
        let target = hostile_name(event.target_id)?;
        // Ignore party rotations; player identities stay in raw evidence rather than activity keys.
        if source.is_none()
            && target.is_none()
            && !matches!(identifier, Identifier::Marker(_) | Identifier::Tether(_))
        {
            continue;
        }
        selected.push((
            index,
            Signal {
                kind: event.kind.clone(),
                identifier,
                source,
                target,
            },
        ));
    }
    Ok(selected)
}

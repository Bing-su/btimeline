use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde_json::json;

use super::super::bosses;
use super::loops::{
    HelperAssociation,
    HelperOccurrence,
    LoopDiscovery,
    LoopModel,
    Member,
    mapping,
};
use super::{Pull, Signal, trace};

type Entity = (i64, i64, i64);
type Owner = (String, i64, i64);

// Whole actor/instance/ability spans distinguish local helpers from continuous background NPC combat.
fn spans(pull: &Pull) -> BTreeMap<Entity, (i64, i64)> {
    let mut result = BTreeMap::new();
    for event in pull
        .events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "cast" | "begincast"))
    {
        let (Some(actor), Some(ability)) = (event.source_id, event.ability_game_id) else {
            continue;
        };
        let at = event.timestamp - pull.start_ms;
        let range = result
            .entry((actor, event.source_instance.unwrap_or(1), ability))
            .or_insert((at, at));
        range.0 = range.0.min(at);
        range.1 = range.1.max(at);
    }
    result
}

fn owner(
    pull: &Pull,
    batch: usize,
    activity: &Signal,
    ranges: &BTreeMap<Entity, (i64, i64)>,
    attempts: &BTreeMap<String, Vec<bosses::Attempt>>,
) -> Option<Owner> {
    let super::super::Identifier::Ability(ability) = activity.identifier else {
        return None;
    };
    let mut result = BTreeSet::new();
    for event in pull
        .batches
        .get(batch)?
        .indices
        .iter()
        .filter_map(|index| pull.events.get(*index))
        .filter(|event| {
            event.kind == "cast"
                && event.ability_game_id == Some(ability)
                && event
                    .source_id
                    .and_then(|id| pull.actors.get(&id))
                    .filter(|name| !name.trim().is_empty())
                    == activity.source.as_ref()
        })
    {
        let actor = event.source_id?;
        if pull.boss_ids.contains(&actor) {
            return None;
        }
        let (first, last) = ranges.get(&(actor, event.source_instance.unwrap_or(1), ability))?;
        let candidates: Vec<_> = attempts
            .iter()
            .flat_map(|(source, attempts)| attempts.iter().map(move |attempt| (source, attempt)))
            .filter(|(_, attempt)| {
                attempt.start_ms <= *first
                    && *last <= attempt.end_ms.unwrap_or(attempt.last_cast_ms)
            })
            .collect();
        let [(source, attempt)] = candidates.as_slice() else {
            return None;
        };
        result.insert(((*source).clone(), attempt.actor, attempt.instance));
    }
    (result.len() == 1)
        .then(|| result.into_iter().next())
        .flatten()
}

fn gather(
    model: &LoopModel,
    pulls: &[&Pull],
) -> Result<BTreeMap<(Signal, usize), Vec<HelperOccurrence>>> {
    let mut result = BTreeMap::<_, Vec<_>>::new();
    let core: BTreeSet<_> = model.body.iter().flatten().collect();
    for episode in &model.episodes {
        let pull = pulls
            .iter()
            .find(|pull| trace(pull) == episode.trace)
            .context("Missing helper trace")?;
        let ranges = spans(pull);
        let attempts = bosses::attempts(pull);
        let positions = mapping(episode, &model.body)
            .context("Missing helper loop correspondence")?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        let times = episode
            .waves
            .iter()
            .map(|wave| wave.at())
            .collect::<Result<Vec<_>>>()?;
        let candidates = pull
            .batches
            .iter()
            .enumerate()
            .flat_map(|(batch, frame)| {
                frame
                    .waves()
                    .into_iter()
                    .map(move |activity| (batch, frame.at_ms, activity))
            })
            .filter(|(_, _, activity)| activity.kind == "cast" && !core.contains(activity));
        let mut counts = BTreeMap::<_, usize>::new();
        for (batch, at_ms, activity) in candidates {
            let Some(index) = times.partition_point(|at| *at <= at_ms).checked_sub(1) else {
                continue;
            };
            let iteration = index / model.body.len() + 1;
            let start = (iteration - 1) * model.body.len();
            let position = *positions.get(index).context("Missing helper context")?;
            let Some((source, actor, instance)) = owner(pull, batch, activity, &ranges, &attempts)
            else {
                continue;
            };
            if model.source.as_ref() != Some(&source)
                || episode.actor != Some(actor)
                || episode.instance != Some(instance)
            {
                continue;
            }
            let cycle_start_ms = *times.get(start).context("Missing helper cycle origin")?;
            // Local ordinal preserves multiple effects without requiring identical interleaving.
            let ordinal = counts.entry((iteration, activity.clone())).or_default();
            *ordinal += 1;
            let occurrence = HelperOccurrence {
                trace: episode.trace.clone(),
                actor: episode.actor,
                instance: episode.instance,
                iteration,
                preceding_position: position,
                cycle_start_ms,
                members: vec![Member {
                    signal: activity.clone(),
                    at_ms,
                    batch,
                }],
            };
            result
                .entry((activity.clone(), *ordinal))
                .or_default()
                .push(occurrence);
        }
    }
    Ok(result)
}

fn missing(model: &LoopModel, observations: &[HelperOccurrence]) -> BTreeMap<String, Vec<usize>> {
    let mut result = BTreeMap::<_, Vec<_>>::new();
    for episode in &model.episodes {
        for iteration in 1..=episode.waves.len().saturating_sub(1) / model.body.len() {
            if !observations.iter().any(|row| {
                row.trace == episode.trace
                    && row.actor == episode.actor
                    && row.instance == episode.instance
                    && row.iteration == iteration
            }) {
                result
                    .entry(episode.trace.clone())
                    .or_default()
                    .push(iteration);
            }
        }
    }
    result
}

// These are display associations, not newly trusted runtime sources; small timing/order changes are fine.
pub(super) fn associate(pulls: &[&Pull], discovery: &mut LoopDiscovery) -> Result<()> {
    for model in discovery
        .models
        .iter_mut()
        .filter(|model| model.source.is_some())
    {
        for ((activity, occurrence), observations) in gather(model, pulls)? {
            let traces: BTreeSet<_> = observations.iter().map(|row| &row.trace).collect();
            let reports: BTreeSet<_> = observations
                .iter()
                .filter_map(|row| row.trace.split_once('/').map(|(report, _)| report))
                .collect();
            let unobserved = missing(model, &observations);
            // Two pulls support a useful forecast; separate reports remain audit metadata.
            let status = if traces.len() < 2 {
                "insufficient supporting pulls"
            } else if observations.iter().all(|row| row.iteration == 1) && unobserved.len() >= 2 {
                "possible one-time or optional effect"
            } else {
                "associated display"
            };
            discovery.diagnostics.push(json!({"method":"helper-display-context-v1","source":model.source,"activity":activity,"occurrence":occurrence,"status":status,"reports":reports.len(),"unobserved_complete_cycles":unobserved,"assumption":"a local effect belongs to this cycle; no helper execution eligibility inferred"}));
            if status == "associated display" {
                model.helper_associations.push(HelperAssociation {
                    activity,
                    occurrence,
                    observations,
                    unobserved_complete_cycles: unobserved,
                });
            }
        }
    }
    Ok(())
}

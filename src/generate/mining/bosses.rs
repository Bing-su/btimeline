use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Value, json};

use super::Pull;

#[derive(Debug, Serialize)]
pub(super) struct Attempt {
    pub(super) actor: i64,
    pub(super) instance: i64,
    pub(super) start_ms: i64,
    start_event: usize,
    pub(super) last_cast_ms: i64,
    pub(super) end_ms: Option<i64>,
    completion_event: Option<usize>,
}

pub(super) fn attempts(pull: &Pull) -> BTreeMap<String, Vec<Attempt>> {
    let mut starts = BTreeMap::<_, ((i64, usize), i64)>::new();
    for (index, event) in pull.events.iter().enumerate() {
        let Some(actor) = event.source_id.filter(|id| pull.boss_ids.contains(id)) else {
            continue;
        };
        if !matches!(event.kind.as_str(), "cast" | "begincast") {
            continue;
        }
        let candidate = (event.timestamp - pull.start_ms, index);
        let span = starts
            .entry((actor, event.source_instance.unwrap_or(1)))
            .or_insert((candidate, candidate.0));
        span.0 = span.0.min(candidate);
        span.1 = span.1.max(candidate.0);
    }
    let mut ends = BTreeMap::<_, (i64, usize)>::new();
    for (index, event) in pull.events.iter().enumerate() {
        let Some(actor) = event.target_id.filter(|id| pull.boss_ids.contains(id)) else {
            continue;
        };
        let dead = event.kind == "death"
            || (event.kind == "damage"
                && event
                    .extra
                    .get("targetResources")
                    .and_then(|r| r.get("hitPoints"))
                    .and_then(Value::as_i64)
                    == Some(0));
        if !dead {
            continue;
        }
        // Missing instance identifies one actor only; it cannot resolve which of several respawns died.
        let instances: Vec<_> = starts
            .keys()
            .filter(|(id, _)| *id == actor)
            .map(|(_, instance)| *instance)
            .collect();
        let instance = event
            .extra
            .get("targetInstance")
            .and_then(Value::as_i64)
            .or_else(|| {
                (instances.len() == 1)
                    .then(|| instances.first().copied())
                    .flatten()
            });
        let Some(instance) = instance else { continue };
        let candidate = (event.timestamp - pull.start_ms, index);
        if starts
            .get(&(actor, instance))
            .is_some_and(|((start, _), _)| *start <= candidate.0)
        {
            let end = ends.entry((actor, instance)).or_insert(candidate);
            *end = (*end).min(candidate);
        }
    }
    let mut result = BTreeMap::<_, Vec<_>>::new();
    for ((actor, instance), ((start_ms, start_event), last_cast_ms)) in starts {
        let Some(name) = pull.actors.get(&actor) else {
            continue;
        };
        let end = ends.get(&(actor, instance));
        result.entry(name.clone()).or_default().push(Attempt {
            actor,
            instance,
            start_ms,
            start_event,
            last_cast_ms,
            end_ms: end.map(|(at, _)| *at),
            completion_event: end.map(|(_, index)| *index),
        });
    }
    result
}

pub(super) fn concurrent(attempts: &[Attempt]) -> bool {
    attempts.iter().enumerate().any(|(index, a)| {
        attempts.iter().skip(index + 1).any(|b| {
            a.start_ms <= b.end_ms.unwrap_or(b.last_cast_ms)
                && b.start_ms <= a.end_ms.unwrap_or(a.last_cast_ms)
        })
    })
}

// Learning uses an observed completed boss attempt; raw graphs and original batch/event indices stay intact.
pub(super) fn project(pulls: &[&Pull]) -> (Vec<Pull>, Vec<Value>) {
    let mut views = Vec::new();
    let mut diagnostics = Vec::new();
    for pull in pulls {
        let mut view = (*pull).clone();
        let mut blocked = BTreeSet::new();
        for (source, attempts) in attempts(pull) {
            let completed: Vec<_> = attempts
                .iter()
                .filter_map(|attempt| attempt.end_ms.map(|end| (attempt, end)))
                .collect();
            let status = match completed.as_slice() {
                _ if concurrent(&attempts) => "overlapping-boss-entities; no attempt selection",
                [(attempt, end)] => {
                    view.boss_scopes
                        .insert(source.clone(), (attempt.start_ms, *end));
                    "observed-boss-completion"
                }
                [] if attempts.len() == 1 => "single-entity-without-observed-completion",
                _ => {
                    blocked.insert(source.clone());
                    "unresolved-attempt-boundaries"
                }
            };
            diagnostics.push(json!({"method":"boss-attempt-scope-v1","trace":format!("{}/{}",pull.report,pull.fight),"source":source,
                "status":status,"attempts":attempts,"selected":view.boss_scopes.get(&source),"raw_observations":"preserved; only the learning view is scoped"}));
        }
        for batch in &mut view.batches {
            batch.signals.retain(|signal| {
                !signal
                    .source
                    .as_ref()
                    .is_some_and(|source| blocked.contains(source))
                    && signal
                        .source
                        .as_ref()
                        .and_then(|source| view.boss_scopes.get(source))
                        .is_none_or(|(start, end)| *start <= batch.at_ms && batch.at_ms <= *end)
            });
        }
        views.push(view);
    }
    (views, diagnostics)
}

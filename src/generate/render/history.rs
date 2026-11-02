use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use kdl::{KdlNode, KdlValue};
use serde_json::{Value, json};

use super::{
    Analysis,
    Anchor,
    CommonEvent,
    Identifier,
    Mode,
    Progression,
    Rendered,
    Signal,
    anchor,
    prop,
    schedule,
    signal,
    timing,
};
use crate::generate::mining::Permutation;

type Key = Vec<usize>;
type Samples = BTreeMap<String, i64>;
type States = BTreeMap<Key, Samples>;
type Edges = BTreeMap<(Key, Key), BTreeSet<String>>;

fn key(path: &[usize], width: usize) -> Key {
    if path.len() == width {
        vec![width, *path.last().unwrap_or(&0)]
    } else {
        path.to_vec()
    }
}

// Merge only finished histories: they have no remaining choices whose correlation could be lost.
fn states(model: &Permutation) -> Result<(States, Edges)> {
    let mut states = States::from([(Vec::new(), model.starts.clone())]);
    let mut edges = Edges::new();
    for (trace, path) in &model.paths {
        let mut previous = Vec::new();
        for (index, block) in path.iter().enumerate() {
            let current = key(
                path.get(..=index).context("Missing history prefix")?,
                model.blocks.len(),
            );
            states.entry(current.clone()).or_default().insert(
                trace.clone(),
                *model
                    .blocks
                    .get(*block)
                    .and_then(|block| block.origins.get(trace))
                    .context("Missing block origin")?,
            );
            edges
                .entry((previous, current.clone()))
                .or_default()
                .insert(trace.clone());
            previous = current;
        }
    }
    Ok((states, edges))
}

fn ability(activity: &Signal) -> Result<String> {
    let Identifier::Ability(id) = activity.identifier else {
        anyhow::bail!("History entry requires an ability")
    };
    Ok(format!("{id:X}"))
}

fn entry(activity: &Signal, from: &str) -> Result<KdlNode> {
    let mut node = signal(
        "enter",
        "Ability",
        &[
            ("id", &ability(activity)?),
            (
                "source",
                activity
                    .source
                    .as_deref()
                    .context("Missing history source")?,
            ),
        ],
    );
    prop(&mut node, "from", from);
    Ok(node)
}

fn phase(id: &str) -> KdlNode {
    let mut node = KdlNode::new("phase");
    node.push(id);
    node
}

fn identifier(model: &Permutation, state: &[usize], prefix: &str) -> Result<String> {
    if state.is_empty() {
        return Ok(format!("{prefix}-entry"));
    }
    let last = *state.last().context("Empty history")?;
    let label = ability(&model.blocks.get(last).context("Missing block")?.trigger)?.to_lowercase();
    let visited = if state.first() == Some(&model.blocks.len()) {
        "complete".into()
    } else {
        state
            .iter()
            .map(|index| {
                ability(&model.blocks.get(*index).context("Missing block")?.trigger)
                    .map(|id| id.to_lowercase())
            })
            .collect::<Result<Vec<_>>>()?
            .join("-")
    };
    Ok(format!("{prefix}-{visited}-at-{label}"))
}

fn rows(
    analysis: &Analysis,
    progression: &Progression,
    events: &[CommonEvent],
    origins: &Samples,
    id: &str,
) -> Result<(Vec<KdlNode>, Vec<Value>)> {
    let anchors = events
        .iter()
        .map(|event| {
            let mut event = event.clone();
            event.samples.retain(|trace, _| origins.contains_key(trace));
            anchor(analysis, progression, &event)
        })
        .collect::<Result<Vec<_>>>()?;
    let (nodes, mut evidence) = schedule(&anchors, origins, Mode::Raid, Some(id), progression)?;
    for row in &mut evidence {
        row.as_object_mut()
            .context("Invalid history evidence")?
            .insert("phase".into(), id.into());
    }
    Ok((nodes, evidence))
}

fn split(
    anchors: &[Anchor],
    progression: &Progression,
    model: &Permutation,
) -> Result<(Vec<Anchor>, Vec<Anchor>)> {
    let mut before = Vec::new();
    let mut after = Vec::new();
    for anchor in anchors {
        let samples = progression.samples(&anchor.observations)?;
        let stages = samples
            .iter()
            .map(|(trace, at)| {
                let left = model.starts.get(trace).context("Missing history start")?;
                let right = model.ends.get(trace).context("Missing history end")?;
                Ok(if at < left {
                    0
                } else if at < right {
                    1
                } else {
                    2
                })
            })
            .collect::<Result<BTreeSet<_>>>()?;
        ensure!(stages.len() == 1, "Finite event crosses history boundary");
        match stages.first() {
            Some(0) => before.push(anchor.clone()),
            Some(2) => after.push(anchor.clone()),
            _ => {}
        }
    }
    Ok((before, after))
}

fn selection(
    model: &Permutation,
    from: &Key,
    to: &Key,
    traces: &BTreeSet<String>,
    states: &States,
    prefix: &str,
) -> Result<KdlNode> {
    let block = *to.last().context("Missing target block")?;
    let mut node = entry(
        &model.blocks.get(block).context("Missing block")?.trigger,
        &identifier(model, from, prefix)?,
    )?;
    let target: Samples = states
        .get(to)
        .context("Missing target state")?
        .iter()
        .filter(|(trace, _)| traces.contains(*trace))
        .map(|(trace, at)| (trace.clone(), *at))
        .collect();
    let origins = states.get(from).context("Missing history origin")?;
    let estimate = timing::estimate(&target, origins)?;
    // A recognized history locates the next block; close before another unused block or continuation.
    let mut close = i64::MAX;
    for (trace, origin) in origins {
        let at = *model
            .blocks
            .get(block)
            .and_then(|b| b.origins.get(trace))
            .context("Missing block time")?;
        let mut end = *model.ends.get(trace).context("Missing continuation time")?;
        let next = model
            .paths
            .get(trace)
            .and_then(|path| path.get(from.len()))
            .context("Missing next observed block")?;
        if *next != block {
            end = end.min(at);
        }
        for (index, other) in model.blocks.iter().enumerate() {
            if index != block
                && !from.contains(&index)
                && let Some(next) = other.origins.get(trace).filter(|next| **next > at)
            {
                end = end.min(*next);
            }
        }
        close = close.min(end.saturating_sub(*origin).saturating_add(50) / 100 - 1);
    }
    ensure!(
        close > estimate.max_tick,
        "No history window before competing progression"
    );
    prop(&mut node, "at", estimate.tick as f64 / 10.0);
    prop(&mut node, "before", estimate.tick as f64 / 10.0);
    prop(&mut node, "after", (close - estimate.tick) as f64 / 10.0);
    Ok(node)
}

fn preserve_choices(
    existing: &Rendered,
    model: &Permutation,
    combat: &mut KdlNode,
) -> Result<BTreeSet<String>> {
    // Existing choices may remain before the region, but must not silently keep a combat clock after it.
    let begin = model
        .starts
        .values()
        .min()
        .copied()
        .context("Missing history start")?;
    let end = model
        .ends
        .values()
        .max()
        .copied()
        .context("Missing history end")?;
    let mut retained = BTreeSet::new();
    for node in existing
        .nodes
        .iter()
        .flat_map(|node| node.children().into_iter().flat_map(|body| body.nodes()))
        .filter(|node| node.name().value() == "branch")
    {
        let open = node
            .get("open")
            .and_then(KdlValue::as_float)
            .context("Missing branch opening")?
            * 1000.0;
        let close = node
            .get("close")
            .and_then(KdlValue::as_float)
            .context("Missing branch close")?
            * 1000.0;
        if close < begin as f64 {
            retained.insert(
                node.get(0)
                    .and_then(KdlValue::as_string)
                    .context("Missing branch ID")?
                    .to_owned(),
            );
            let body = combat.ensure_children().nodes_mut();
            let position = body
                .iter()
                .position(|event| {
                    event
                        .get(1)
                        .and_then(KdlValue::as_float)
                        .is_some_and(|at| at * 1000.0 > open)
                })
                .unwrap_or(body.len());
            body.insert(position, node.clone());
        } else {
            ensure!(
                open >= begin as f64 && close <= end as f64,
                "Existing choice crosses history scope"
            );
        }
    }
    Ok(retained)
}

fn block_phases(
    analysis: &Analysis,
    progression: &Progression,
    model: &Permutation,
    prefix: &str,
    states: &States,
    edges: &Edges,
) -> Result<Rendered> {
    let mut result = Rendered::default();
    for (state, origins) in states {
        let id = identifier(model, state, prefix)?;
        let mut node = phase(&id);
        let events = if let Some(block) = state.last() {
            for ((from, to), traces) in edges.iter().filter(|((_, to), _)| to == state) {
                node.ensure_children()
                    .nodes_mut()
                    .push(selection(model, from, to, traces, states, prefix)?);
            }
            model
                .blocks
                .get(*block)
                .context("Missing block")?
                .events
                .clone()
        } else {
            node.ensure_children()
                .nodes_mut()
                .push(entry(&model.left, "combat")?);
            vec![CommonEvent {
                signal: model.left.clone(),
                samples: model.starts.clone(),
                occurrence: Some(1),
                evidence: json!({"method":"history-region-entry-v1","sync":"omitted: phase entry"}),
            }]
        };
        let (events, mut rows) = rows(analysis, progression, &events, origins, &id)?;
        node.ensure_children().nodes_mut().extend(events);
        result.events.append(&mut rows);
        result.nodes.push(node);
        result.origins.push(
            json!({"phase":id,"policy":"observed-block-history","samples":origins,"history":state}),
        );
    }
    Ok(result)
}

fn build(
    analysis: &Analysis,
    progression: &Progression,
    model: &Permutation,
    anchors: &[Anchor],
    existing: &Rendered,
) -> Result<Rendered> {
    ensure!(
        existing.loops.is_empty(),
        "History inside a rendered loop is unsupported"
    );
    let prefix = format!("order-after-{}", ability(&model.left)?.to_lowercase());
    let (states, edges) = states(model)?;
    ensure!(states.len() <= 64, "History state limit");
    let (before, after) = split(anchors, progression, model)?;
    let zeros: Samples = model.starts.keys().map(|id| (id.clone(), 0)).collect();
    let (nodes, mut evidence) = schedule(&before, &zeros, Mode::Raid, None, progression)?;
    let mut combat = phase("combat");
    combat.ensure_children().nodes_mut().push(signal(
        "enter",
        "InCombat",
        &[("inGameCombat", "1")],
    ));
    combat.ensure_children().nodes_mut().extend(nodes);
    let branch_ids = preserve_choices(existing, model, &mut combat)?;
    let mut result = block_phases(analysis, progression, model, &prefix, &states, &edges)?;
    result.nodes.insert(0, combat);
    let controls = existing
        .nodes
        .iter()
        .filter(|node| node.name().value() != "phase" && node.name().value() != "event")
        .cloned();
    result.nodes.splice(0..0, controls);
    result.events.append(&mut evidence);
    result
        .origins
        .push(json!({"phase":"combat","policy":"combat-start","samples":zeros}));
    let id = format!("{prefix}-continuation");
    let mut continuation = phase(&id);
    for state in states.keys() {
        continuation
            .ensure_children()
            .nodes_mut()
            .push(entry(&model.right, &identifier(model, state, &prefix)?)?);
    }
    let (nodes, mut rows) = schedule(&after, &model.ends, Mode::Raid, Some(&id), progression)?;
    for row in &mut rows {
        row.as_object_mut()
            .context("Invalid continuation evidence")?
            .insert("phase".into(), id.clone().into());
    }
    continuation.ensure_children().nodes_mut().extend(nodes);
    result.events.append(&mut rows);
    result.nodes.push(continuation);
    result
        .origins
        .push(json!({"phase":id,"policy":"history-continuation","samples":model.ends}));
    result.events.extend(
        existing
            .events
            .iter()
            .filter(|e| {
                e["branch"]
                    .as_str()
                    .is_some_and(|id| branch_ids.contains(id))
            })
            .cloned(),
    );
    retain_observations(&existing.events, &result.events)?;
    result.histories.push(json!({"method":"bounded-block-history-v1","status":"rendered","model":model,"states":states.keys().collect::<Vec<_>>(),"edges":edges.keys().collect::<Vec<_>>(),"window_policy":"state start until observed competing progression","not_checked":["unseen paths","ACT runtime"]}));
    Ok(result)
}

// Adding a state model must not silently discard a finite or option display that was already supported.
fn retain_observations(previous: &[Value], current: &[Value]) -> Result<()> {
    let mut covered = BTreeSet::new();
    for row in current {
        for (trace, node) in row["observation_nodes"]
            .as_object()
            .context("Missing history observations")?
        {
            covered.insert((trace, node.as_u64().context("Invalid observation node")?));
        }
    }
    for row in previous {
        for (trace, node) in row["observation_nodes"]
            .as_object()
            .context("Missing finite observations")?
        {
            ensure!(
                covered.contains(&(trace, node.as_u64().context("Invalid finite node")?)),
                "History model omits an existing display observation: {}",
                row["event"]
            );
        }
    }
    Ok(())
}

pub(super) fn apply(
    analysis: &Analysis,
    progression: &Progression,
    models: &[Permutation],
    anchors: &[Anchor],
    rendered: &mut Rendered,
) {
    for model in models {
        match build(analysis,progression,model,anchors,rendered) {
            Ok(result)=>*rendered=result,
            Err(error)=>rendered.histories.push(json!({"method":"bounded-block-history-v1","status":"unrendered","reason":error.to_string(),"model":model})),
        }
    }
}

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use petgraph::stable_graph::NodeIndex;
use serde_json::{Value, json};

use super::{Candidate, Draft, Progression, Pull, Route, Signal, common, prefixes};
use crate::generate::mining::occurrences::{Branch, BranchOption, Event, branches};

pub(in crate::generate) struct Choice {
    pub after: Option<usize>,
    pub join: usize,
    pub source: String,
    pub options: Vec<Route>,
}

pub(super) fn activities(candidate: &Candidate, progression: &Progression) -> Result<Vec<Signal>> {
    let trace = progression
        .traces
        .get(candidate.trace)
        .context("Missing choice trace")?;
    candidate
        .nodes
        .iter()
        .map(|(node, _)| {
            trace
                .graph
                .node_weight(NodeIndex::new(*node))
                .map(|node| node.activity.clone())
                .context("Missing choice occurrence")
        })
        .collect()
}

fn slice(candidate: &Candidate, nodes: Vec<(usize, i64)>) -> Candidate {
    Candidate {
        trace: candidate.trace,
        reference: candidate.reference.clone(),
        nodes,
        observed_span_ms: candidate.observed_span_ms,
        end_ms: candidate.end_ms,
    }
}

type Variants<'a> = BTreeMap<Vec<Signal>, Vec<&'a Candidate>>;

fn variants<'a>(
    base: &[Signal],
    paths: &[Vec<Signal>],
    candidates: &'a [Candidate],
    prefix: usize,
    end: usize,
    join: &Signal,
) -> Result<Option<Variants<'a>>> {
    let mut variants = BTreeMap::<Vec<Signal>, Vec<&Candidate>>::new();
    let mut expected = base
        .get(prefix..end)
        .context("Invalid choice interval")?
        .to_vec();
    expected.sort();
    for (peer, path) in candidates.iter().zip(paths) {
        let block = path.get(prefix..end).context("Missing choice block")?;
        let mut sorted = block.to_vec();
        sorted.sort();
        let ordered = peer
            .nodes
            .get(prefix.saturating_sub(1)..=end)
            .context("Missing choice boundary")?
            .windows(2)
            .all(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|((_, a), (_, b))| {
                        *b > a.saturating_add(common::ORDER_RESOLUTION_MS)
                    })
            });
        if sorted != expected
            || !ordered
            || block.iter().any(|signal| signal.source != join.source)
            || path.iter().filter(|signal| *signal == join).count() != 1
        {
            return Ok(None);
        }
        variants.entry(block.to_vec()).or_default().push(peer);
    }
    let triggers: Vec<_> = variants.keys().filter_map(|block| block.first()).collect();
    if triggers.len() < 2
        || triggers.iter().copied().collect::<BTreeSet<_>>().len() != triggers.len()
        || paths.iter().any(|path| {
            triggers
                .iter()
                .any(|trigger| path.iter().filter(|signal| signal == trigger).count() != 1)
        })
    {
        return Ok(None);
    }
    Ok(Some(variants))
}

// Keep correlated changes together, e.g. X -> shared -> Y / Y -> shared -> X never produces X -> shared -> X.
pub(super) fn discover(
    source: &Option<String>,
    reference: &Candidate,
    candidates: &[Candidate],
    progression: &Progression,
) -> Result<Option<(Route, Vec<Value>)>> {
    if source.is_some()
        || candidates
            .iter()
            .any(|peer| peer.nodes.len() > reference.nodes.len())
    {
        return Ok(None);
    }
    let complete: Vec<_> = candidates
        .iter()
        .filter(|peer| peer.nodes.len() == reference.nodes.len())
        .map(|peer| slice(peer, peer.nodes.clone()))
        .collect();
    let Some((mut route, mut diagnostics)) =
        discover_complete(source, reference, &complete, progression)?
    else {
        return Ok(None);
    };
    for peer in candidates
        .iter()
        .filter(|peer| peer.nodes.len() < reference.nodes.len())
    {
        diagnostics.push(prefixes::bind(&mut route, peer, &complete, progression)?);
    }
    route.refresh_relations(progression)?;
    Ok(Some((route, diagnostics)))
}

fn discover_complete(
    source: &Option<String>,
    reference: &Candidate,
    candidates: &[Candidate],
    progression: &Progression,
) -> Result<Option<(Route, Vec<Value>)>> {
    // shortcut: learn raid permutations first; add boss origins and unequal full routes with their own tests.
    if source.is_some()
        || candidates.len() < 2
        || candidates
            .iter()
            .any(|peer| peer.nodes.len() != reference.nodes.len())
    {
        return Ok(None);
    }
    let paths = candidates
        .iter()
        .map(|peer| activities(peer, progression))
        .collect::<Result<Vec<_>>>()?;
    let base = activities(reference, progression)?;
    let prefix = base
        .iter()
        .enumerate()
        .take_while(|(index, signal)| paths.iter().all(|path| path.get(*index) == Some(*signal)))
        .count();
    if prefix == base.len() {
        return Ok(None);
    }
    let suffix = base
        .iter()
        .rev()
        .enumerate()
        .take(base.len() - prefix)
        .take_while(|(offset, signal)| {
            paths
                .iter()
                .all(|path| path.get(base.len() - 1 - offset) == Some(*signal))
        })
        .count();
    if suffix == 0 {
        return Ok(None);
    }
    let end = base.len() - suffix;
    let join = base.get(end).context("Missing shared continuation")?;
    let Some(caster) = &join.source else {
        return Ok(None);
    };
    let Some(variants) = variants(&base, &paths, candidates, prefix, end, join)? else {
        return Ok(None);
    };
    let mut options = Vec::new();
    let mut diagnostics = Vec::new();
    for peers in variants.into_values() {
        let selected = peers
            .iter()
            .map(|peer| {
                Ok(slice(
                    peer,
                    peer.nodes
                        .get(prefix..end)
                        .context("Missing option occurrences")?
                        .to_vec(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let seed = selected.first().context("Missing option reference")?;
        let (route, checks) = Route::learn(None, seed, &selected, progression)?;
        options.push(route);
        diagnostics.extend(checks);
    }
    let backbone = candidates
        .iter()
        .map(|peer| {
            slice(
                peer,
                peer.nodes
                    .iter()
                    .take(prefix)
                    .chain(peer.nodes.iter().skip(end))
                    .copied()
                    .collect(),
            )
        })
        .collect::<Vec<_>>();
    let seed = backbone
        .iter()
        .find(|peer| peer.reference == reference.reference)
        .context("Missing backbone reference")?;
    let (mut route, checks) = Route::learn(None, seed, &backbone, progression)?;
    diagnostics.extend(checks);
    diagnostics.push(json!({"method":"representative-order-choice-v1","reference":reference.reference,"source":caster,
        "status":"learned","prefix_nodes":prefix,"suffix_nodes":suffix,"options":options.len()}));
    route.choices.push(Choice {
        after: prefix.checked_sub(1),
        join: prefix,
        source: caster.clone(),
        options,
    });
    Ok(Some((route, diagnostics)))
}

pub(super) fn lower(
    draft: &Draft,
    pulls: &[Pull],
    progression: &Progression,
) -> Result<Vec<Branch>> {
    let mut result = Vec::new();
    for route in &draft.routes {
        for choice in &route.choices {
            let join = route
                .graph
                .node_weight(NodeIndex::new(choice.join))
                .context("Missing choice join")?;
            let ends = progression.samples(&join.observations)?;
            let options = choice.options.iter().map(|path| {
                let trigger = path.graph.node_weights().next().context("Missing choice trigger")?;
                let events = path.graph.node_weights().map(|node| Ok(Event { signal: node.activity.clone(),
                    // Unequal prefix coverage can invert medians; use joined runs for the entire body schedule.
                    samples: progression.samples(&node.observations)?.into_iter().filter(|(trace,_)| ends.contains_key(trace)).collect(),
                    occurrence: Some(node.occurrence),
                    evidence: json!({"method":"representative-order-option-v1","reference":path.reference,
                        "timing_policy":"observed-option-join","supported_observation_nodes":node.observations}) })).collect::<Result<Vec<_>>>()?;
                Ok(BranchOption { trigger: trigger.activity.clone(), triggers: progression.samples(&trigger.observations)?, events })
            }).collect::<Result<Vec<_>>>()?;
            let (open, close) = branches::window(
                &options,
                &pulls.iter().collect::<Vec<_>>(),
                &ends,
                choice.after.is_none(),
            )?;
            result.push(Branch {
                source: choice.source.clone(),
                join: join.activity.clone(),
                open,
                close,
                options,
            });
        }
    }
    Ok(result)
}

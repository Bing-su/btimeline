use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use petgraph::algo::is_cyclic_directed;
use petgraph::graph::DiGraph;
use serde_json::{Value, json};

use super::{Common, Event, Group, Pull, Signal, alignment, trace};

type Token<'a> = (&'a Signal, usize);

fn tokens<'a>(row: &[Group<'a>]) -> Option<Vec<(Token<'a>, usize)>> {
    let mut counts = BTreeMap::new();
    let mut result = Vec::new();
    for (index, group) in row.iter().enumerate() {
        for signal in &group.key {
            group.unique(signal)?;
            let ordinal = counts.entry(*signal).or_insert(0);
            *ordinal += 1;
            result.push(((*signal, *ordinal), index));
        }
    }
    Some(result)
}

// Union only observed inter-group order; unordered peers add no invented edges inside their wave.
fn compatible<'a>(rows: &[&[Group<'a>]]) -> Option<Vec<Vec<(Token<'a>, usize)>>> {
    let paths = rows
        .iter()
        .map(|row| tokens(row))
        .collect::<Option<Vec<_>>>()?;
    let reference = paths.first()?;
    if !alignment::within_limit(reference.len(), reference.len()) {
        return None;
    }
    let mut graph = DiGraph::<(), ()>::new();
    let nodes: BTreeMap<_, _> = reference
        .iter()
        .map(|(token, _)| (*token, graph.add_node(())))
        .collect();
    let mut work = 0;
    for (row, path) in rows.iter().zip(&paths) {
        if path.len() != reference.len() {
            return None;
        }
        let mut groups = vec![Vec::new(); row.len()];
        for (token, index) in path {
            groups.get_mut(*index)?.push(*nodes.get(token)?);
        }
        for pair in groups.windows(2) {
            let [left, right] = pair else { return None };
            work += left.len().checked_mul(right.len())?;
            if work > alignment::MAX_CELLS {
                return None;
            }
            for (from, to) in left
                .iter()
                .flat_map(|from| right.iter().map(move |to| (from, to)))
            {
                graph.update_edge(*from, *to, ());
            }
        }
    }
    (!is_cyclic_directed(&graph)).then_some(paths)
}

// Equal per-signal counts plus an acyclic union identify occurrences without requiring identical waves.
pub(super) fn partial(
    pulls: &[&Pull],
    rows: &[&[Group<'_>]],
    bounds: &Value,
) -> Result<Vec<Event>> {
    let Some(paths) = compatible(rows) else {
        return Ok(Vec::new());
    };
    let reference = paths.first().context("Missing partial-order reference")?;
    let mut result = Vec::new();
    for (token, _) in reference {
        let groups = paths
            .iter()
            .zip(rows)
            .map(|(path, row)| {
                let (_, index) = path
                    .iter()
                    .find(|(other, _)| other == token)
                    .context("Missing partial-order occurrence")?;
                row.get(*index).context("Missing partial-order group")
            })
            .collect::<Result<Vec<_>>>()?;
        for mut event in super::events(pulls, &groups, bounds)?
            .into_iter()
            .filter(|event| &event.signal == token.0)
        {
            let fields = event
                .evidence
                .as_object_mut()
                .context("Invalid partial-order evidence")?;
            fields.insert("method".into(), "consistent-partial-order-v1".into());
            // Entry/branch callers may label their enclosing algorithm; retain this underlying proof.
            fields.insert(
                "correspondence_policy".into(),
                "consistent-partial-order-v1".into(),
            );
            result.push(event);
        }
    }
    Ok(result)
}

// A missing companion must not hide a shared cast, e.g. A {B,C} D / A B D still supports B.
// Only completed bounded/opening intervals use this view; censored tails and loop proofs keep all keys.
pub(super) fn shared(
    pulls: &[&Pull],
    rows: &mut [Vec<Group<'_>>],
    bounds: &Value,
    result: &mut Common,
) {
    let keys: Vec<BTreeSet<_>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .flat_map(|group| group.key.iter().copied())
                .collect()
        })
        .collect();
    let Some(first) = keys.first() else { return };
    let common: BTreeSet<_> = first
        .iter()
        .copied()
        .filter(|signal| keys.iter().all(|keys| keys.contains(signal)))
        .collect();
    for (pull, row) in pulls.iter().zip(rows) {
        let removed: Vec<_> = row.iter().flat_map(|group|&group.observations)
            .filter(|observation|!common.contains(observation.signal))
            .map(|observation|json!({"activity":observation.signal,"batch":observation.batch,"at_ms":observation.at})).collect();
        if removed.is_empty() {
            continue;
        }
        result.diagnostics.push(json!({"method":"shared-activity-interval-v1","bounds":bounds,"trace":trace(pull),"excluded_from_alignment":removed,"raw_observations":"preserved; missing companions do not establish a branch"}));
        for group in row.iter_mut() {
            group.key.retain(|signal| common.contains(signal));
        }
        row.retain(|group| !group.key.is_empty());
    }
}

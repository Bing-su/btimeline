use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use petgraph::unionfind::UnionFind;
use serde::Serialize;
use serde_json::{Value, json};

use super::{Boundary, Common, Event, Group, Pull, Signal, fragments, span, trace};

type Samples = BTreeMap<String, i64>;
type Table = BTreeMap<Signal, Vec<i64>>;

fn sample(common: &BTreeMap<Signal, Vec<i64>>, signal: &Signal, trace: usize) -> Result<i64> {
    common
        .get(signal)
        .and_then(|times| times.get(trace))
        .copied()
        .context("Missing block landmark time")
}

#[derive(Serialize)]
pub(in crate::generate) struct Block {
    pub trigger: Signal,
    pub origins: Samples,
    pub events: Vec<Event>,
}

#[derive(Serialize)]
pub(in crate::generate) struct Permutation {
    pub left: Signal,
    pub right: Signal,
    pub starts: Samples,
    pub ends: Samples,
    pub blocks: Vec<Block>,
    pub paths: BTreeMap<String, Vec<usize>>,
}

fn table(pull: &Pull) -> Table {
    super::super::timing_table(pull)
        .into_iter()
        .filter(|(signal, _)| signal.kind == "cast" && signal.source.is_some())
        .collect()
}

fn once(table: &Table, signal: &Signal) -> Option<i64> {
    let [at] = table.get(signal)?.as_slice() else {
        return None;
    };
    Some(*at)
}

fn common(tables: &[Table]) -> BTreeMap<Signal, Vec<i64>> {
    super::super::common::anchors(tables)
}

// Stable order joins landmarks inside a block; independently moving blocks remain disconnected.
fn blocks(common: &BTreeMap<Signal, Vec<i64>>) -> Result<Vec<Vec<Signal>>> {
    ensure!(common.len() <= 128, "Permutation landmark limit");
    let entries: Vec<_> = common.iter().collect();
    let mut sets = UnionFind::<usize>::new(entries.len());
    for (i, (_, a)) in entries.iter().enumerate() {
        for (j, (_, b)) in entries.iter().enumerate().skip(i + 1) {
            if a.iter().zip(*b).all(|(a, b)| a.saturating_add(100) < *b)
                || a.iter().zip(*b).all(|(a, b)| b.saturating_add(100) < *a)
            {
                sets.union(i, j);
            }
        }
    }
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for (i, (signal, _)) in entries.iter().enumerate() {
        groups
            .entry(sets.find(i))
            .or_default()
            .push((*signal).clone());
    }
    let mut result: Vec<_> = groups.into_values().collect();
    for group in &mut result {
        group.sort_by_key(|signal| common.get(signal).and_then(|times| times.first()));
    }
    result.sort_by_key(|group| group.first().cloned());
    // Two-way local order choices already have a renderer; this model handles longer permutations.
    ensure!(
        (3..=6).contains(&result.len()),
        "Not a multi-block permutation"
    );
    Ok(result)
}

fn paths(
    common: &BTreeMap<Signal, Vec<i64>>,
    blocks: &[Vec<Signal>],
    count: usize,
) -> Result<Vec<Vec<usize>>> {
    let mut result = Vec::new();
    for trace in 0..count {
        let mut landmarks: Vec<_> = common.keys().collect();
        landmarks.sort_by_key(|signal| common.get(*signal).and_then(|times| times.get(trace)));
        let mut path = Vec::new();
        for signal in landmarks {
            let block = blocks
                .iter()
                .position(|group| group.contains(signal))
                .context("Missing block")?;
            if path.last() != Some(&block) {
                path.push(block);
            }
        }
        ensure!(path.len() == blocks.len(), "Block landmarks interleave");
        // The same entry must lead its block in every trace, even if peers reorder internally.
        for (index, group) in blocks.iter().enumerate() {
            let first = group.first().context("Empty block")?;
            let start = sample(common, first, trace)?;
            ensure!(
                group
                    .iter()
                    .skip(1)
                    .all(|signal| sample(common, signal, trace)
                        .is_ok_and(|at| at > start.saturating_add(100))),
                "Ambiguous block entry"
            );
            ensure!(path.contains(&index), "Missing block");
        }
        result.push(path);
    }
    Ok(result)
}

// A once-per-block identity has a known occurrence even when two peer casts exchange order.
fn unique_displays(pulls: &[&Pull], rows: &[Vec<Group<'_>>], bounds: &Value) -> Result<Vec<Event>> {
    let Some(first) = rows.first() else {
        return Ok(Vec::new());
    };
    let keys: BTreeSet<_> = first
        .iter()
        .flat_map(|group| group.key.iter().copied())
        .collect();
    let mut result = Vec::new();
    for signal in keys {
        let groups = rows
            .iter()
            .map(|row| {
                let mut matches = row.iter().filter(|group| group.key.contains(&signal));
                match (matches.next(), matches.next()) {
                    (Some(group), None) if group.unique(signal).is_some() => Some(group),
                    _ => None,
                }
            })
            .collect::<Option<Vec<_>>>();
        let Some(groups) = groups else { continue };
        for mut event in super::events(pulls, &groups, bounds)?
            .into_iter()
            .filter(|event| event.signal == *signal)
        {
            let fields = event
                .evidence
                .as_object_mut()
                .context("Invalid unique block evidence")?;
            fields.insert("method".into(), "unique-block-activity-v1".into());
            fields.insert(
                "relative_order".into(),
                "not inferred by unique identity; display only".into(),
            );
            result.push(event);
        }
    }
    Ok(result)
}

fn body(pulls: &[&Pull], origins: &Samples, ends: &Samples) -> Result<Vec<Event>> {
    let mut rows = pulls
        .iter()
        .map(|pull| {
            let id = trace(pull);
            span(
                pull,
                origins[&id] - 1,
                Some(ends[&id]),
                &None,
                &|signal| signal.kind == "cast" && signal.source.is_some(),
                Boundary::Display,
            )
            .map_err(anyhow::Error::msg)
        })
        .collect::<Result<Vec<_>>>()?;
    let bounds = json!({"origin":"history-block-entry","boundary_policy":"display-interior-v1"});
    let mut learned = Common {
        events: Vec::new(),
        branches: Vec::new(),
        diagnostics: Vec::new(),
    };
    super::projection::shared(pulls, &mut rows, &bounds, &mut learned);
    fragments::normalize(&mut rows)?;
    super::expand_interval(
        pulls,
        &rows.iter().map(Vec::as_slice).collect::<Vec<_>>(),
        bounds.clone(),
        &mut learned,
    )?;
    for event in unique_displays(pulls, &rows, &bounds)? {
        if !learned
            .events
            .iter()
            .any(|existing| existing.signal == event.signal && existing.samples == event.samples)
        {
            learned.events.push(event);
        }
    }
    ensure!(!learned.events.is_empty(), "No block displays");
    // Encounter-wide ordinals change with block order; display IDs count occurrences inside the block.
    learned.events.sort_by(|a, b| {
        a.signal
            .cmp(&b.signal)
            .then_with(|| a.samples.values().next().cmp(&b.samples.values().next()))
    });
    let mut ordinals = BTreeMap::new();
    for event in &mut learned.events {
        let ordinal = ordinals.entry(event.signal.clone()).or_insert(0);
        *ordinal += 1;
        event.occurrence = Some(*ordinal);
        event
            .evidence
            .as_object_mut()
            .context("Invalid block evidence")?
            .insert("occurrence_scope".into(), "history-block".into());
    }
    Ok(learned.events)
}

fn candidate(
    pulls: &[&Pull],
    all_tables: &[Table],
    left: &Signal,
    right: &Signal,
) -> Result<Permutation> {
    let mut scopes: Vec<_> = pulls
        .iter()
        .zip(all_tables)
        .filter_map(|(pull, times)| {
            let (start, end) = (once(times, left)?, once(times, right)?);
            (start.saturating_add(100) < end).then_some((*pull, start, end, times))
        })
        .collect();
    // A deterministic reference keeps display IDs stable when shared actions change global ordinal.
    scopes.sort_by_key(|(pull, _, _, _)| trace(pull));
    ensure!(
        scopes.len() >= 3
            && pulls
                .iter()
                .filter(|p| p.kill)
                .all(|pull| scopes.iter().any(|(p, _, _, _)| trace(p) == trace(pull))),
        "Incomplete permutation bounds"
    );
    let tables: Vec<_> = scopes
        .iter()
        .map(|(_, start, end, times)| {
            times
                .iter()
                .map(|(signal, times)| {
                    (
                        signal.clone(),
                        times
                            .iter()
                            .copied()
                            .filter(|at| {
                                start.saturating_add(100) < *at && *at < end.saturating_sub(100)
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .filter(|(_, times)| !times.is_empty())
                .collect()
        })
        .collect();
    let common = common(&tables);
    let groups = blocks(&common)?;
    let orders = paths(&common, &groups, scopes.len())?;
    ensure!(
        orders.iter().collect::<BTreeSet<_>>().len() >= 3,
        "Too few observed block orders"
    );
    let mut blocks = Vec::new();
    let scoped: Vec<_> = scopes.iter().map(|(pull, _, _, _)| *pull).collect();
    for (block, group) in groups.iter().enumerate() {
        let trigger = group.first().context("Empty block")?.clone();
        let mut origins = Samples::new();
        let mut ends = Samples::new();
        for (index, (pull, _, end, _)) in scopes.iter().enumerate() {
            let order = orders.get(index).context("Missing observed order")?;
            let position = order
                .iter()
                .position(|b| *b == block)
                .context("Missing block position")?;
            let next = order
                .get(position + 1)
                .and_then(|b| groups.get(*b))
                .and_then(|g| g.first());
            origins.insert(trace(pull), sample(&common, &trigger, index)?);
            ends.insert(
                trace(pull),
                match next {
                    Some(next) => sample(&common, next, index)?,
                    None => *end,
                },
            );
        }
        let events = body(&scoped, &origins, &ends)?;
        blocks.push(Block {
            trigger,
            origins,
            events,
        });
    }
    Ok(Permutation {
        left: left.clone(),
        right: right.clone(),
        starts: scopes.iter().map(|(p, s, _, _)| (trace(p), *s)).collect(),
        ends: scopes.iter().map(|(p, _, e, _)| (trace(p), *e)).collect(),
        blocks,
        paths: scopes
            .iter()
            .zip(orders)
            .map(|((p, _, _, _), path)| (trace(p), path))
            .collect(),
    })
}

// Search bounded observed regions, not whole-log variants; short wipes may simply lack both bounds.
pub(in crate::generate::mining) fn discover(pulls: &[&Pull]) -> Result<Vec<Permutation>> {
    let all_tables: Vec<_> = pulls.iter().map(|p| table(p)).collect();
    let tables: Vec<_> = pulls
        .iter()
        .zip(&all_tables)
        .filter(|(p, _)| p.kill)
        .map(|(_, times)| times.clone())
        .collect();
    let checkpoints = common(&tables);
    if checkpoints.len() > 64 {
        return Ok(Vec::new());
    }
    let mut best: Option<Permutation> = None;
    let mut best_length = i64::MAX;
    for (left, a) in &checkpoints {
        for (right, b) in &checkpoints {
            if !a.iter().zip(b).all(|(a, b)| a.saturating_add(100) < *b) {
                continue;
            }
            let Ok(model) = candidate(pulls, &all_tables, left, right) else {
                continue;
            };
            let length = model
                .ends
                .values()
                .zip(model.starts.values())
                .map(|(b, a)| b - a)
                .min()
                .unwrap_or(i64::MAX);
            if length < best_length {
                best_length = length;
                best = Some(model);
            }
        }
    }
    Ok(best.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::generate::mining::Identifier;

    proptest! {
        #[test]
        fn moving_blocks_do_not_depend_on_ability_numbers_or_clock_scale(base in 1_i64..100000, gap in 1000_i64..10000, jitter in 0_i64..50) {
            let orders=[[0,1,2],[0,2,1],[1,0,2],[1,2,0],[2,0,1],[2,1,0]];
            let common=(0..6).map(|index| {
                let signal=Signal{kind:"cast".into(),identifier:Identifier::Ability(base+index),source:Some("Synthetic source".into()),target:None};
                let times=orders.iter().enumerate().map(|(trace,order)| {
                    order.iter().position(|block|*block==index/2).expect("block") as i64*gap+(index%2)*200+trace as i64*jitter
                }).collect();
                (signal,times)
            }).collect();
            let result=blocks(&common).expect("three independently moving blocks");
            prop_assert_eq!(result.len(),3);
            prop_assert!(result.iter().all(|group|group.len()==2));
            prop_assert_eq!(paths(&common,&result,6).expect("contiguous blocks").len(),6);
        }
    }
}

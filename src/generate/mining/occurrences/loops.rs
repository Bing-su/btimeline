use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use petgraph::unionfind::UnionFind;
use serde::Serialize;
use serde_json::{Value, json};

use super::super::bosses;
use super::{Boundary, Group, Pull, Signal, fragments, interval, trace};

#[derive(Clone, Debug, Serialize)]
pub(in crate::generate) struct Member {
    pub signal: Signal,
    pub at_ms: i64,
    pub batch: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::generate) struct Wave {
    pub key: Vec<Signal>,
    pub members: Vec<Member>,
    coalescible: bool,
}

impl Wave {
    pub fn at(&self) -> Result<i64> {
        self.members
            .first()
            .map(|m| m.at_ms)
            .context("Empty loop wave")
    }
}

struct Scope {
    trace: String,
    actor: i64,
    instance: i64,
    source: String,
    waves: Vec<Wave>,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::generate) struct Episode {
    pub trace: String,
    pub actor: i64,
    pub instance: i64,
    pub phase_origin_ms: i64,
    pub waves: Vec<Wave>,
}

#[derive(Clone, Debug, Serialize)]
pub(in crate::generate) struct LoopModel {
    pub source: String,
    pub prefix: Vec<Vec<Signal>>,
    pub body: Vec<Vec<Signal>>,
    pub blocks: Vec<Vec<usize>>,
    pub episodes: Vec<Episode>,
}

#[derive(Default)]
pub(in crate::generate) struct LoopDiscovery {
    pub models: Vec<LoopModel>,
    pub diagnostics: Vec<Value>,
}

fn wave(group: Group<'_>) -> Wave {
    let key: Vec<_> = group
        .key
        .iter()
        .copied()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let coalescible = key.iter().all(|signal| {
        group.key.iter().filter(|s| ***s == *signal).count() == 1
            || fragments::disjoint(&group, signal)
    });
    Wave {
        key,
        coalescible,
        members: group
            .observations
            .into_iter()
            .map(|row| Member {
                signal: row.signal.clone(),
                at_ms: row.at,
                batch: row.batch,
            })
            .collect(),
    }
}

fn scopes(pulls: &[Pull], diagnostics: &mut Vec<Value>) -> Result<Vec<Scope>> {
    let mut result = Vec::new();
    for pull in pulls {
        for (source, attempts) in bosses::attempts(pull) {
            if bosses::concurrent(&attempts) {
                diagnostics.push(json!({"method":"terminal-loop-v1","trace":trace(pull),"source":source,"status":"unresolved","reason":"concurrent body instances"}));
                continue;
            }
            for attempt in attempts {
                let groups = interval(
                    pull,
                    attempt.start_ms - 1,
                    Some(attempt.end_ms.unwrap_or(attempt.last_cast_ms) + 1),
                    &Some(source.clone()),
                    &|s| s.kind == "cast",
                    Boundary::Display,
                );
                match groups {
                    Ok(groups)=>result.push(Scope{trace:trace(pull),actor:attempt.actor,instance:attempt.instance,source:source.clone(),waves:groups.into_iter().map(wave).collect()}),
                    Err(reason)=>diagnostics.push(json!({"method":"terminal-loop-v1","trace":trace(pull),"source":source,"status":"unresolved","reason":reason})),
                }
            }
        }
    }
    result.sort_by_key(|scope| {
        (
            scope.source.clone(),
            scope.trace.clone(),
            scope.actor,
            scope.instance,
        )
    });
    Ok(result)
}

fn accepts(wave: &Wave, key: &Vec<Signal>) -> bool {
    wave.coalescible && &wave.key == key
}

// Fixed opener context identifies onset; a later repeated ID cannot replace a missing earlier step.
fn episode(scope: &Scope, prefix: &[Vec<Signal>], body: &[Vec<Signal>]) -> Option<Episode> {
    if scope.waves.len() <= prefix.len() {
        return None;
    }
    if !scope
        .waves
        .iter()
        .zip(prefix)
        .all(|(wave, key)| accepts(wave, key))
    {
        return None;
    }
    let waves = scope.waves.get(prefix.len()..)?;
    if !accepts(waves.first()?, body.first()?) {
        return None;
    }
    Some(Episode {
        trace: scope.trace.clone(),
        actor: scope.actor,
        instance: scope.instance,
        phase_origin_ms: scope.waves.first()?.at().ok()?,
        waves: waves.to_vec(),
    })
}

fn mapping(episode: &Episode, body: &[Vec<Signal>]) -> Option<Vec<Vec<usize>>> {
    episode
        .waves
        .chunks(body.len())
        .map(|chunk| {
            let positions = chunk
                .iter()
                .map(|wave| body.iter().position(|key| accepts(wave, key)))
                .collect::<Option<Vec<_>>>()?;
            (positions.first() == Some(&0)
                && positions.iter().copied().collect::<BTreeSet<_>>().len() == positions.len())
            .then_some(positions)
        })
        .collect()
}

// Connected positional displacements form contiguous permutation blocks, e.g. A X Y B / A Y X B.
fn blocks(body_len: usize, paths: &[Vec<Vec<usize>>]) -> Result<Vec<Vec<usize>>> {
    let mut sets = UnionFind::<usize>::new(body_len);
    for path in paths.iter().flatten() {
        for (position, mapped) in path.iter().enumerate() {
            for step in position.min(*mapped)..position.max(*mapped) {
                sets.union(step, step + 1);
            }
        }
    }
    let mut groups = BTreeMap::<_, Vec<_>>::new();
    for position in 0..body_len {
        groups
            .entry(sets.find(position))
            .or_default()
            .push(position);
    }
    let mut result: Vec<_> = groups.into_values().collect();
    result.sort_by_key(|block| block.first().copied());
    for block in result.iter().filter(|block| block.len() > 1) {
        let end = *block.last().context("Empty permutation block")? + 1;
        let witnessed = paths
            .iter()
            .flatten()
            .filter(|path| path.len() >= end)
            .any(|path| {
                block
                    .iter()
                    .filter_map(|index| path.get(*index))
                    .copied()
                    .collect::<BTreeSet<_>>()
                    == block.iter().copied().collect()
                    && block.iter().any(|index| path.get(*index) != Some(index))
            });
        ensure!(
            witnessed,
            "Permutation block has no complete reordered witness"
        );
    }
    Ok(result)
}

fn candidate(scopes: &[&Scope], reference: &Scope, start: usize, end: usize) -> Result<LoopModel> {
    let prefix: Vec<_> = reference
        .waves
        .iter()
        .take(start)
        .map(|wave| wave.key.clone())
        .collect();
    let body: Vec<_> = reference
        .waves
        .get(start..end)
        .context("Invalid loop range")?
        .iter()
        .map(|wave| wave.key.clone())
        .collect();
    // shortcut: distinct wave identities only; repeated internal keys need context-aware alignment.
    ensure!(
        body.len() >= 2 && body.iter().collect::<BTreeSet<_>>().len() == body.len(),
        "Loop positions require distinct wave identities"
    );
    // Every base position has an unfragmented witness; coalescing peers still checks disjoint entities.
    ensure!(
        reference
            .waves
            .iter()
            .take(end)
            .all(|wave| wave.coalescible && wave.key.len() == wave.members.len()),
        "Fragmented loop reference"
    );
    let mut episodes = Vec::new();
    for scope in scopes {
        if scope.waves.len() <= prefix.len()
            && scope
                .waves
                .iter()
                .zip(&prefix)
                .all(|(wave, key)| accepts(wave, key))
        {
            continue;
        }
        let ep = episode(scope, &prefix, &body).context("Opener or loop entry differs")?;
        episodes.push(ep);
    }
    let paths = episodes
        .iter()
        .map(|ep| mapping(ep, &body).context("Terminal path differs from loop body"))
        .collect::<Result<Vec<_>>>()?;
    let supported: BTreeSet<_> = episodes
        .iter()
        .filter(|ep| ep.waves.len() >= body.len() + 3)
        .map(|ep| &ep.trace)
        .collect();
    ensure!(
        supported.len() >= 2,
        "Loop needs a complete cycle and next two steps in two input traces"
    );
    let blocks = blocks(body.len(), &paths)?;
    Ok(LoopModel {
        source: reference.source.clone(),
        prefix,
        body,
        blocks,
        episodes,
    })
}

fn ranges(scope: &Scope) -> Vec<(usize, usize)> {
    let mut previous = BTreeMap::new();
    let mut result = Vec::new();
    for (end, wave) in scope
        .waves
        .iter()
        .enumerate()
        .filter(|(_, wave)| wave.key.len() == 1)
    {
        if let Some(start) = previous.insert(wave.key.clone(), end) {
            result.push((start, end));
        }
    }
    result
}

pub(super) fn discover(pulls: &[Pull]) -> Result<LoopDiscovery> {
    let mut result = LoopDiscovery::default();
    let all = scopes(pulls, &mut result.diagnostics)?;
    let sources: BTreeSet<_> = all.iter().map(|scope| &scope.source).collect();
    for source in sources {
        if result
            .diagnostics
            .iter()
            .any(|row| row["source"] == **source && row["status"] == "unresolved")
        {
            continue;
        }
        let scopes: Vec<_> = all.iter().filter(|scope| &scope.source == source).collect();
        let cost: usize = scopes.iter().map(|scope| scope.waves.len()).sum();
        let mut work = 0_usize;
        let mut candidates = Vec::new();
        for (scope, start, end) in scopes.iter().flat_map(|scope| {
            ranges(scope)
                .into_iter()
                .map(move |(start, end)| (*scope, start, end))
        }) {
            work = work.saturating_add(cost);
            if work > super::alignment::MAX_CELLS {
                break;
            }
            if let Ok(model) = candidate(&scopes, scope, start, end) {
                candidates.push(model);
            }
        }
        // Earliest structurally supported onset keeps the finite opener small; timing is estimated later.
        candidates.sort_by_key(|model| (model.prefix.len(), std::cmp::Reverse(model.body.len())));
        if let Some(model) = candidates.into_iter().next() {
            result.diagnostics.push(json!({"method":"terminal-loop-v1","source":source,"status":"learned","steps":model.body.len(),"opener_waves":model.prefix.len(),"episodes":model.episodes.len()}));
            result.models.push(model);
        } else {
            result.diagnostics.push(json!({"method":"terminal-loop-v1","source":source,"status":"unresolved","reason":"no shared terminal recurrence with supported onset and continuation"}));
        }
        if work > super::alignment::MAX_CELLS {
            result.diagnostics.push(
                json!({"method":"terminal-loop-v1","source":source,"status":"resource-limit"}),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::generate::mining::Identifier;

    fn scope(trace: &str, ids: &[i64], step: i64) -> Scope {
        Scope {
            trace: trace.into(),
            actor: 10,
            instance: 1,
            source: "Boss".into(),
            waves: ids
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    let signal = Signal {
                        kind: "cast".into(),
                        identifier: Identifier::Ability(*id),
                        source: Some("Boss".into()),
                        target: None,
                    };
                    Wave {
                        key: vec![signal.clone()],
                        members: vec![Member {
                            signal,
                            at_ms: index as i64 * step,
                            batch: index,
                        }],
                        coalescible: true,
                    }
                })
                .collect(),
        }
    }

    #[rstest::rstest]
    #[case(&[9, 1, 2, 3, 4, 1, 3, 2], true)]
    #[case(&[1, 2, 3, 4, 1, 2, 3], false)] // Missing opener.
    #[case(&[9, 1, 2, 4, 1, 2, 3], false)] // Missing internal step.
    #[case(&[9, 1, 2, 3, 4, 1, 2], false)] // No following two steps.
    #[case(&[9, 1, 2, 3, 4, 2, 1, 3], false)] // Shifted cycle boundary.
    #[case(&[9, 1, 2, 3, 4, 1, 2, 3, 8], false)] // Unknown terminal exit.
    fn complete_context_is_required(#[case] peer: &[i64], #[case] accepted: bool) {
        let reference = scope("a", &[9, 1, 2, 3, 4, 1, 2, 3], 1000);
        let peer = scope("b", peer, 1000);
        assert_eq!(
            candidate(&[&reference, &peer], &reference, 1, 5).is_ok(),
            accepted
        );
    }

    #[test]
    fn whole_permutation_blocks_keep_their_anchor_context() {
        let a = scope("a", &[9, 1, 2, 3, 4, 1, 2, 3], 1000);
        let b = scope("b", &[9, 1, 3, 2, 4, 1, 2, 3], 1000);
        let short = scope("short", &[9], 1000);
        let model = candidate(&[&a, &b, &short], &a, 1, 5).unwrap();
        assert_eq!(model.blocks, vec![vec![0], vec![1, 2], vec![3]]);
        assert_eq!(model.episodes.len(), 2);
        blocks(4, &[vec![vec![0, 2]]]).unwrap_err();
        candidate(&[&a, &a], &a, 1, 5).unwrap_err();
    }

    proptest! {
        #[test]
        fn time_stretch_does_not_change_the_progression(step in 1_i64..100_000) {
            let a = scope("a", &[9, 1, 2, 3, 4, 1, 2, 3], 1000);
            let b = scope("b", &[9, 1, 3, 2, 4, 1, 2, 3], step);
            let model = candidate(&[&a, &b], &a, 1, 5).unwrap();
            prop_assert_eq!(model.blocks, vec![vec![0], vec![1, 2], vec![3]]);
            prop_assert_eq!(model.episodes[1].waves[0].at().unwrap(), step);
        }
    }
}

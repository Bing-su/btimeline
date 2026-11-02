use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{Candidate, LoopModel, Progression, Route, Signal};

mod validation;

#[derive(Serialize)]
pub(in crate::generate) struct Repeat {
    entry: Option<usize>,
    body: Vec<Vec<Signal>>,
    blocks: Vec<Vec<usize>>,
    // Keep chronological raw waves, e.g. a final partial cycle never receives invented observations.
    observed_waves: BTreeMap<String, Vec<Vec<usize>>>,
}

impl Repeat {
    pub(super) fn bind(mut self, route: &Route) -> Result<Self> {
        self.entry = route.graph.node_indices().find_map(|index| {
            let node = route.graph.node_weight(index)?;
            self.observed_waves
                .iter()
                .any(|(trace, waves)| {
                    waves
                        .first()
                        .is_some_and(|wave| wave.first() == node.observations.get(trace))
                })
                .then_some(index.index())
        });
        ensure!(
            self.entry.is_some(),
            "Terminal repeat has no representative entry"
        );
        Ok(self)
    }
}

// Reuse structurally verified episodes; only their timestamps belong to the separate loop scheduler.
pub(super) fn prepare(
    model: &LoopModel,
    candidates: &mut [Candidate],
    progression: &Progression,
) -> Result<Option<Repeat>> {
    // shortcut: one episode per trace and a single observed entry; upgrade with explicit attempt scopes and compound entries.
    if model
        .episodes
        .iter()
        .any(|ep| ep.waves.first().is_none_or(|wave| wave.members.len() != 1))
        || model
            .episodes
            .iter()
            .map(|ep| &ep.trace)
            .collect::<BTreeSet<_>>()
            .len()
            != model.episodes.len()
    {
        return Ok(None);
    }
    let observed_waves: BTreeMap<_, _> = model
        .episodes
        .iter()
        .map(|episode| {
            let waves = episode
                .waves
                .iter()
                .map(|wave| {
                    wave.members
                        .iter()
                        .map(|member| {
                            progression
                                .bind(
                                    &member.signal,
                                    &BTreeMap::from([(episode.trace.clone(), member.at_ms)]),
                                )?
                                .get(&episode.trace)
                                .copied()
                                .context("Missing repeat observation")
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((episode.trace.clone(), waves))
        })
        .collect::<Result<_>>()?;
    // A loop detector may project a narrower scope; never discard a trailing raw occurrence it did not explain.
    for peer in candidates.iter() {
        if let Some(waves) = observed_waves.get(&peer.reference) {
            let covered: BTreeSet<_> = waves.iter().flatten().copied().collect();
            let entry = waves
                .first()
                .and_then(|wave| wave.first())
                .context("Missing repeat boundary")?;
            let Some(start) = peer.nodes.iter().position(|(node, _)| node == entry) else {
                return Ok(None);
            };
            if peer
                .nodes
                .iter()
                .skip(start)
                .any(|(node, _)| !covered.contains(node))
            {
                return Ok(None);
            }
        }
    }
    for peer in candidates {
        if let Some(waves) = observed_waves.get(&peer.reference) {
            let entry = waves
                .first()
                .and_then(|wave| wave.first())
                .context("Missing repeat boundary")?;
            let start = peer
                .nodes
                .iter()
                .position(|(node, _)| node == entry)
                .context("Missing candidate boundary")?;
            peer.nodes.truncate(start + 1);
        }
    }
    Ok(Some(Repeat {
        entry: None,
        body: model.body.clone(),
        blocks: model.blocks.clone(),
        observed_waves,
    }))
}

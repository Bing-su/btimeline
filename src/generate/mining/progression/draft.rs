use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde_json::{Value, json};

use super::{Progression, Pull, Signal, common, covers};
use crate::generate::mining::bosses;

pub(in crate::generate) struct Node {
    pub activity: Signal,
    pub occurrence: usize,
    pub observations: BTreeMap<String, usize>,
}

pub(in crate::generate) struct Route {
    pub source: Option<String>,
    pub reference: String,
    pub graph: StableDiGraph<Node, (), usize>,
    relations_complete: bool,
    peers_complete: bool,
}

pub(in crate::generate) struct Draft {
    pub routes: Vec<Route>,
    diagnostics: Vec<Value>,
}

struct Candidate {
    trace: usize,
    reference: String,
    nodes: Vec<(usize, i64)>,
    observed_span_ms: i64,
}

impl Candidate {
    fn new(
        pull: &Pull,
        progression: &Progression,
        source: &Option<String>,
    ) -> Result<Option<Self>> {
        if source.as_ref().is_some_and(|name| {
            bosses::attempts(pull)
                .get(name)
                .is_some_and(|attempts| bosses::concurrent(attempts))
        }) {
            return Ok(None);
        }
        let reference = format!("{}/{}", pull.report, pull.fight);
        let trace = progression
            .traces
            .iter()
            .position(|trace| trace.trace == reference)
            .context("Missing draft source trace")?;
        let raw = progression
            .traces
            .get(trace)
            .context("Invalid draft trace")?;
        let times = progression
            .samples
            .get(&reference)
            .context("Missing draft observations")?;
        let mut nodes = Vec::new();
        for index in raw.graph.node_indices() {
            let node = raw.graph.node_weight(index).context("Invalid draft node")?;
            if node.unclassified
                || source
                    .as_ref()
                    .is_some_and(|name| node.activity.source.as_ref() != Some(name))
                || !pull
                    .batches
                    .get(node.observation)
                    .is_some_and(|batch| batch.signals.contains(&node.activity))
            {
                continue;
            }
            nodes.push((
                index.index(),
                *times
                    .get(index.index())
                    .context("Missing draft timestamp")?,
            ));
        }
        let (Some((_, first)), Some((_, last))) = (nodes.first(), nodes.last()) else {
            return Ok(None);
        };
        let observed_span_ms = last - if source.is_some() { *first } else { 0 };
        Ok(Some(Self {
            trace,
            reference,
            nodes,
            observed_span_ms,
        }))
    }
}

impl Draft {
    // Start with one observed route, e.g. a long wipe retains mechanics absent in a fast clear.
    pub(in crate::generate::mining) fn new(
        pulls: &[Pull],
        progression: &Progression,
        boss_only: bool,
    ) -> Result<Self> {
        let classified: Vec<_> = pulls.iter().map(Pull::learning_view).collect();
        let mut selected: Vec<_> = classified.iter().map(|pull| pull.as_ref()).collect();
        selected
            .sort_by(|left, right| (&left.report, left.fight).cmp(&(&right.report, right.fight)));
        let (views, mut diagnostics) = if boss_only {
            bosses::project(&selected)
        } else {
            (Vec::new(), Vec::new())
        };
        let pulls: Vec<_> = if boss_only {
            views.iter().collect()
        } else {
            selected
        };
        let sources: BTreeSet<_> = if boss_only {
            pulls
                .iter()
                .flat_map(|pull| pull.boss_names.iter().cloned().map(Some))
                .collect()
        } else {
            BTreeSet::from([None])
        };
        let mut routes = Vec::new();
        for source in sources {
            let candidates: Vec<_> = pulls
                .iter()
                .map(|pull| Candidate::new(pull, progression, &source))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect();
            diagnostics.extend(candidates.iter().map(|candidate|
                json!({"method":"representative-candidate-v1","source":source,"trace":candidate.reference,
                    "observed_span_ms":candidate.observed_span_ms,"cast_waves":candidate.nodes.len(),"coverage":"unconfirmed"})));
            let Some(candidate) = candidates.iter().min_by_key(|candidate| {
                (
                    Reverse(candidate.observed_span_ms),
                    Reverse(candidate.nodes.len()),
                    candidate.reference.clone(),
                )
            }) else {
                diagnostics.push(json!({"method":"representative-route-v1","source":source,"status":"unresolved","reason":"no unambiguous eligible scope"}));
                continue;
            };
            let (route, checks) = Route::learn(source, candidate, &candidates, progression)?;
            routes.push(route);
            diagnostics.extend(checks);
        }
        Ok(Self {
            routes,
            diagnostics,
        })
    }

    pub(in crate::generate) fn supports_observed_routes(&self) -> bool {
        !self.routes.is_empty() && self.routes.iter().all(|route| route.peers_complete)
    }

    pub(in crate::generate) fn evidence(&self) -> Value {
        let routes: Vec<_> = self.routes.iter().map(|route| {
            let nodes: Vec<_> = route.graph.node_indices().filter_map(|index| route.graph.node_weight(index).map(|node|
                json!({"index":index.index(),"activity":node.activity,"occurrence":node.occurrence,
                    "observation_nodes":node.observations}))).collect();
            let mut before: Vec<_> = route.graph.edge_references().map(|edge| [edge.source().index(),edge.target().index()]).collect();
            before.sort_unstable();
            json!({"source":route.source,"reference":route.reference,"nodes":nodes,"before":before,
                "relations_complete":route.relations_complete,"peers_complete":route.peers_complete,"coverage":"unconfirmed"})
        }).collect();
        json!({"method":"representative-route-v1","scope":"one observed route per combat or boss; alternatives and loops not inferred",
            "routes":routes,"diagnostics":self.diagnostics})
    }
}

impl Route {
    fn learn(
        source: Option<String>,
        candidate: &Candidate,
        candidates: &[Candidate],
        progression: &Progression,
    ) -> Result<(Self, Vec<Value>)> {
        let mut diagnostics = Vec::new();
        let raw = progression
            .traces
            .get(candidate.trace)
            .context("Invalid representative trace")?;
        let mut graph = StableDiGraph::default();
        let mut counts = BTreeMap::new();
        for (observation, _) in &candidate.nodes {
            let activity = raw
                .graph
                .node_weight(NodeIndex::new(*observation))
                .context("Missing representative node")?
                .activity
                .clone();
            let occurrence = counts.entry(activity.clone()).or_insert(0);
            *occurrence += 1;
            graph.add_node(Node {
                activity,
                occurrence: *occurrence,
                observations: BTreeMap::from([(candidate.reference.clone(), *observation)]),
            });
        }
        let mut starts = vec![0; candidate.nodes.len()];
        let mut peers_complete = true;
        for peer in candidates {
            let raw = progression
                .traces
                .get(peer.trace)
                .context("Invalid peer trace")?;
            let matches = graph.node_count() == peer.nodes.len()
                && graph
                    .node_weights()
                    .zip(&peer.nodes)
                    .all(|(node, (observation, _))| {
                        raw.graph
                            .node_weight(NodeIndex::new(*observation))
                            .is_some_and(|peer| peer.activity == node.activity)
                    });
            diagnostics.push(json!({"method":"exact-route-correspondence-v1","source":source,
                    "reference":candidate.reference,"trace":peer.reference,"status":if matches {"matched"} else {"unresolved"},
                    "reason":if matches {"same complete occurrence sequence"} else {"occurrence sequence differs; alternatives or censoring need analysis"}}));
            if !matches {
                peers_complete = false;
                continue;
            }
            let times: Vec<_> = peer.nodes.iter().map(|(_, at)| *at).collect();
            for (node, (observation, _)) in graph.node_weights_mut().zip(&peer.nodes) {
                node.observations
                    .insert(peer.reference.clone(), *observation);
            }
            // Intersect certain precedence across peers, e.g. a near-simultaneous A/B stays unordered.
            for (start, at) in starts.iter_mut().zip(&times) {
                let boundary = at.saturating_add(common::ORDER_RESOLUTION_MS);
                *start = (*start).max(times.partition_point(|time| *time <= boundary));
            }
        }
        let (before, relations_complete) = covers(&starts, 1_000_000);
        for [from, to] in before {
            graph.add_edge(NodeIndex::new(from), NodeIndex::new(to), ());
        }
        Ok((
            Self {
                source,
                reference: candidate.reference.clone(),
                graph,
                relations_complete,
                peers_complete,
            },
            diagnostics,
        ))
    }
}

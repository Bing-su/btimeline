use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde_json::{Value, json};

use super::{Progression, Pull, Signal, relations};
use crate::generate::mining::bosses;

pub(in crate::generate) struct Node {
    pub activity: Signal,
    pub occurrence: usize,
    pub observation: usize,
}

pub(in crate::generate) struct Route {
    pub source: Option<String>,
    pub reference: String,
    pub graph: StableDiGraph<Node, (), usize>,
    relations_complete: bool,
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
            let Some(candidate) = candidates.into_iter().min_by_key(|candidate| {
                (
                    Reverse(candidate.observed_span_ms),
                    Reverse(candidate.nodes.len()),
                    candidate.reference.clone(),
                )
            }) else {
                diagnostics.push(json!({"method":"representative-route-v1","source":source,"status":"unresolved","reason":"no unambiguous eligible scope"}));
                continue;
            };
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
                    observation: *observation,
                });
            }
            // Recompute covers after filtering, e.g. A -> excluded helper -> B must still retain A -> B.
            let times: Vec<_> = candidate.nodes.iter().map(|(_, at)| *at).collect();
            let (before, relations_complete) = relations(&times, 1_000_000);
            for [from, to] in before {
                graph.add_edge(NodeIndex::new(from), NodeIndex::new(to), ());
            }
            routes.push(Route {
                source,
                reference: candidate.reference,
                graph,
                relations_complete,
            });
        }
        Ok(Self {
            routes,
            diagnostics,
        })
    }

    pub(in crate::generate) fn evidence(&self) -> Value {
        let routes: Vec<_> = self.routes.iter().map(|route| {
            let nodes: Vec<_> = route.graph.node_indices().filter_map(|index| route.graph.node_weight(index).map(|node|
                json!({"index":index.index(),"activity":node.activity,"occurrence":node.occurrence,
                    "observation_nodes":{route.reference.clone():node.observation}}))).collect();
            let mut before: Vec<_> = route.graph.edge_references().map(|edge| [edge.source().index(),edge.target().index()]).collect();
            before.sort_unstable();
            json!({"source":route.source,"reference":route.reference,"nodes":nodes,"before":before,
                "relations_complete":route.relations_complete,"coverage":"unconfirmed"})
        }).collect();
        json!({"method":"representative-route-v1","scope":"one observed route per combat or boss; alternatives and loops not inferred",
            "routes":routes,"diagnostics":self.diagnostics})
    }
}

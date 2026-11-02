use std::collections::BTreeMap;

use anyhow::{Context, Result};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde::Serialize;

use super::{Pull, Signal, common};

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Node {
    observation: usize,
    activity: Signal,
    occurrence: usize,
    unclassified: bool,
}

#[derive(Debug)]
struct Trace {
    trace: String,
    kill: bool,
    graph: StableDiGraph<Node, (), usize>,
    relations_complete: bool,
}

pub(in crate::generate) struct Progression {
    traces: Vec<Trace>,
    samples: BTreeMap<String, Vec<i64>>,
    lookup: BTreeMap<(String, Signal, i64), usize>,
}

// Preserve only certain precedence, e.g. 0,80,160ms retains 0->160 without ordering its neighbors.
fn relations(times: &[i64], limit: usize) -> (Vec<[usize; 2]>, bool) {
    let mut edges = Vec::new();
    for (from, at) in times.iter().enumerate() {
        let start =
            times.partition_point(|time| *time <= at.saturating_add(common::ORDER_RESOLUTION_MS));
        let Some(first) = times.get(start) else {
            continue;
        };
        let end = times
            .partition_point(|time| *time <= first.saturating_add(common::ORDER_RESOLUTION_MS));
        for to in start..end {
            if edges.len() == limit {
                return (edges, false);
            }
            edges.push([from, to]);
        }
    }
    (edges, true)
}

impl Progression {
    pub(super) fn new(pulls: &[Pull]) -> Self {
        let mut ordered: Vec<_> = pulls.iter().collect();
        ordered.sort_by(|a, b| (&a.report, a.fight).cmp(&(&b.report, b.fight)));
        let mut result = Self {
            traces: Vec::new(),
            samples: BTreeMap::new(),
            lookup: BTreeMap::new(),
        };
        for pull in ordered {
            let trace = format!("{}/{}", pull.report, pull.fight);
            let mut graph = StableDiGraph::<Node, (), usize>::default();
            let mut times = Vec::new();
            let mut counts = BTreeMap::new();
            let waves = pull
                .batches
                .iter()
                .enumerate()
                .flat_map(|(observation, batch)| {
                    batch
                        .waves()
                        .into_iter()
                        .map(move |activity| (observation, batch.at_ms, activity))
                })
                .filter(|(_, _, signal)| signal.kind == "cast");
            for (observation, at, activity) in waves {
                let occurrence = counts.entry(activity).or_insert(0);
                *occurrence += 1;
                let node = graph.add_node(Node {
                    observation,
                    activity: activity.clone(),
                    occurrence: *occurrence,
                    unclassified: pull
                        .batches
                        .get(observation)
                        .is_some_and(|batch| !batch.eligible().contains(activity)),
                });
                result
                    .lookup
                    .insert((trace.clone(), activity.clone(), at), node.index());
                times.push(at);
            }
            let (before, relations_complete) = relations(&times, 1_000_000);
            for [from, to] in before {
                graph.add_edge(NodeIndex::new(from), NodeIndex::new(to), ());
            }
            result.traces.push(Trace {
                trace: trace.clone(),
                kill: pull.kill,
                graph,
                relations_complete,
            });
            result.samples.insert(trace, times);
        }
        result
    }

    pub(in crate::generate) fn bind(
        &self,
        activity: &Signal,
        samples: &BTreeMap<String, i64>,
    ) -> Result<BTreeMap<String, usize>> {
        samples
            .iter()
            .map(|(trace, at)| {
                let node = self
                    .lookup
                    .get(&(trace.clone(), activity.clone(), *at))
                    .context("Missing occurrence graph observation")?;
                Ok((trace.clone(), *node))
            })
            .collect()
    }

    pub(in crate::generate) fn samples(
        &self,
        observations: &BTreeMap<String, usize>,
    ) -> Result<BTreeMap<String, i64>> {
        observations
            .iter()
            .map(|(trace, node)| {
                self.traces
                    .iter()
                    .find(|candidate| &candidate.trace == trace)
                    .and_then(|candidate| candidate.graph.node_weight(NodeIndex::new(*node)))
                    .context("Invalid occurrence graph reference")?;
                let at = self
                    .samples
                    .get(trace)
                    .and_then(|times| times.get(*node))
                    .context("Invalid occurrence graph reference")?;
                Ok((trace.clone(), *at))
            })
            .collect()
    }

    pub(in crate::generate) fn evidence(&self) -> serde_json::Value {
        let traces: Vec<_> = self.traces.iter().map(|trace| {
            let nodes: Vec<_> = trace.graph.node_indices().filter_map(|index| {
                trace.graph.node_weight(index).map(|node| serde_json::json!({
                    "index":index.index(),"observation":node.observation,"activity":node.activity,"occurrence":node.occurrence,"unclassified":node.unclassified
                }))
            }).collect();
            let mut before: Vec<_> = trace.graph.edge_references().map(|edge|[edge.source().index(),edge.target().index()]).collect();
            before.sort_unstable();
            serde_json::json!({"trace":trace.trace,"kill":trace.kill,"nodes":nodes,"before":before,"relations_complete":trace.relations_complete})
        }).collect();
        serde_json::json!({"method":"per-trace-occurrence-order-v1","scope":"cast candidates; not a merged execution workflow",
            "order_resolution_ms":common::ORDER_RESOLUTION_MS,"traces":traces})
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn repeated_occurrences_survive_without_a_unique_anchor_or_estimated_time() {
        let input = camino::Utf8PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/pull.json"
        ));
        let analysis = super::super::analyze(&[input]).unwrap();
        let mut progression = analysis.progression();
        let topology = progression.evidence();
        let trace = progression.traces.first().unwrap();
        assert_eq!(trace.graph.node_count(), 3);
        assert_eq!(
            trace
                .graph
                .node_weights()
                .map(|node| node.occurrence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(
            topology["traces"][0]["before"],
            serde_json::json!([[0, 1], [1, 2]])
        );
        let activity = &trace.graph.node_weights().next().unwrap().activity;
        let binding = progression
            .bind(activity, &BTreeMap::from([(trace.trace.clone(), 1000)]))
            .unwrap();
        let samples = progression.samples(&binding).unwrap();
        let initial = crate::generate::timing::estimate(
            &samples,
            &BTreeMap::from([(trace.trace.clone(), 0)]),
        )
        .unwrap();
        assert_eq!(initial.median_ms, 1000);
        crate::generate::timing::estimate(
            &samples,
            &BTreeMap::from([(trace.trace.clone(), 20000)]),
        )
        .unwrap_err();
        assert_eq!(
            progression.evidence(),
            topology,
            "timing failure must not mutate topology"
        );
        progression
            .samples(&BTreeMap::from([(trace.trace.clone(), usize::MAX)]))
            .unwrap_err();
        progression
            .bind(activity, &BTreeMap::from([(trace.trace.clone(), 123456)]))
            .unwrap_err();
        let id = trace.trace.clone();
        // Removing a candidate must not shift another occurrence's raw timing or evidence reference.
        progression
            .traces
            .first_mut()
            .unwrap()
            .graph
            .remove_node(NodeIndex::new(0));
        progression
            .samples(&BTreeMap::from([(id.clone(), 0)]))
            .unwrap_err();
        assert_eq!(
            progression
                .samples(&BTreeMap::from([(id.clone(), 1)]))
                .unwrap(),
            BTreeMap::from([(id, 5000)])
        );
        assert_eq!(progression.evidence()["traces"][0]["nodes"][0]["index"], 1);
    }

    #[test]
    fn adjacency_does_not_erase_a_certain_longer_range_order() {
        assert_eq!(relations(&[0, 80, 160], 100), (vec![[0, 2]], true));
        assert_eq!(relations(&[0, 0, 101], 100), (vec![[0, 2], [1, 2]], true));
        assert_eq!(relations(&[0, 200, 400], 1), (vec![[0, 1]], false));
    }

    proptest! {
        #[test]
        fn covers_preserve_exactly_the_certain_partial_order(mut times in prop::collection::vec(0_i64..2000,0..20)) {
            times.sort_unstable();
            let (edges,complete) = relations(&times,10000);
            prop_assert!(complete);
            let mut graph = StableDiGraph::<(), (), usize>::default();
            for _ in &times { graph.add_node(()); }
            for [from,to] in edges { graph.add_edge(NodeIndex::new(from),NodeIndex::new(to),()); }
            prop_assert!(petgraph::algo::toposort(&graph,None).is_ok());
            for (a,at) in times.iter().enumerate() {
                for (b,bt) in times.iter().enumerate() {
                    prop_assert_eq!(a!=b && petgraph::algo::has_path_connecting(&graph,NodeIndex::new(a),NodeIndex::new(b),None),*bt>at.saturating_add(common::ORDER_RESOLUTION_MS));
                }
            }
        }
    }
}

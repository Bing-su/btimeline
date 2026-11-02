use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use petgraph::stable_graph::{NodeIndex, StableDiGraph};
use petgraph::visit::{EdgeRef, IntoEdgeReferences};
use serde_json::{Value, json};

use super::{Progression, Pull, Signal, common, covers};
use crate::generate::mining::bosses;

mod choices;
mod prefixes;
mod validation;

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
    pub choices: Vec<choices::Choice>,
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
    end_ms: i64,
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
            end_ms: pull.scope_end(source),
        }))
    }

    // Translate the next observed gap, e.g. A/B ending before C is a prefix, not evidence C is optional.
    fn next_expected_at(&self, reference: &Self) -> Option<i64> {
        let (_, next) = reference.nodes.get(self.nodes.len())?;
        let ((_, previous), (_, at)) = reference.nodes.iter().zip(&self.nodes).next_back()?;
        at.checked_add(next.checked_sub(*previous)?)
    }

    fn same_prefix(
        &self,
        reference: &Self,
        progression: &Progression,
        reference_progression: &Progression,
    ) -> Result<bool> {
        if self.nodes.len() > reference.nodes.len() {
            return Ok(false);
        }
        let raw = progression
            .traces
            .get(self.trace)
            .context("Invalid peer trace")?;
        let original = reference_progression
            .traces
            .get(reference.trace)
            .context("Invalid representative trace")?;
        for ((peer, _), (reference, _)) in self.nodes.iter().zip(&reference.nodes) {
            let peer = raw
                .graph
                .node_weight(NodeIndex::new(*peer))
                .context("Invalid peer node")?;
            let reference = original
                .graph
                .node_weight(NodeIndex::new(*reference))
                .context("Invalid representative node")?;
            if peer.activity != reference.activity {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn learning_views(pulls: &[Pull], boss_only: bool) -> (Vec<Cow<'_, Pull>>, Vec<Value>) {
    let mut views: Vec<_> = pulls.iter().map(Pull::learning_view).collect();
    views.sort_by(|a, b| (&a.report, a.fight).cmp(&(&b.report, b.fight)));
    if boss_only {
        let selected: Vec<_> = views.iter().map(|pull| pull.as_ref()).collect();
        let (projected, diagnostics) = bosses::project(&selected);
        (projected.into_iter().map(Cow::Owned).collect(), diagnostics)
    } else {
        (views, Vec::new())
    }
}

impl Draft {
    pub(in crate::generate::mining) fn validate_choices(
        pulls: &[Pull],
        boss_only: bool,
    ) -> Result<Value> {
        validation::check(pulls, boss_only)
    }

    pub(in crate::generate::mining) fn branches(
        &self,
        pulls: &[Pull],
        progression: &Progression,
    ) -> Result<Vec<crate::generate::mining::Branch>> {
        choices::lower(self, pulls, progression)
    }

    // Start with one observed route, e.g. a long wipe retains mechanics absent in a fast clear.
    pub(in crate::generate::mining) fn new(
        pulls: &[Pull],
        progression: &Progression,
        boss_only: bool,
    ) -> Result<Self> {
        let (pulls, mut diagnostics) = learning_views(pulls, boss_only);
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

    pub(in crate::generate) fn has_choices(&self) -> bool {
        self.routes.iter().any(|route| !route.choices.is_empty())
    }

    pub(in crate::generate) fn evidence(&self) -> Value {
        let routes: Vec<_> = self.routes.iter().map(Route::evidence).collect();
        json!({"method":"representative-route-v1","scope":"representative routes with supported local order choices; general loops not inferred",
            "routes":routes,"diagnostics":self.diagnostics})
    }
}

impl Route {
    // Rebuild after binding a prefix, e.g. near-simultaneous A/B must not erase their certain order before C.
    fn refresh_relations(&mut self, progression: &Progression) -> Result<()> {
        let samples = self
            .graph
            .node_weights()
            .map(|node| progression.samples(&node.observations))
            .collect::<Result<Vec<_>>>()?;
        let traces: BTreeSet<_> = samples.iter().flat_map(BTreeMap::keys).collect();
        let mut starts = vec![0; samples.len()];
        for trace in traces {
            let times: Vec<_> = samples
                .iter()
                .map_while(|sample| sample.get(trace).copied())
                .collect();
            ensure!(
                samples
                    .iter()
                    .skip(times.len())
                    .all(|sample| !sample.contains_key(trace)),
                "Route observations must form a complete path or prefix"
            );
            for (start, at) in starts.iter_mut().zip(&times) {
                let boundary = at.saturating_add(common::ORDER_RESOLUTION_MS);
                *start = (*start).max(times.partition_point(|time| *time <= boundary));
            }
        }
        let (before, complete) = covers(&starts, 1_000_000);
        let indices: Vec<_> = self.graph.node_indices().collect();
        self.graph.clear_edges();
        for [from, to] in before {
            self.graph.add_edge(
                *indices.get(from).context("Missing order source")?,
                *indices.get(to).context("Missing order target")?,
                (),
            );
        }
        self.relations_complete = complete;
        Ok(())
    }

    fn evidence(&self) -> Value {
        let route = self;
        let nodes: Vec<_> = route
            .graph
            .node_indices()
            .filter_map(|index| {
                route.graph.node_weight(index).map(|node|
                json!({"index":index.index(),"activity":node.activity,"occurrence":node.occurrence,
                    "observation_nodes":node.observations}))
            })
            .collect();
        let mut before: Vec<_> = route
            .graph
            .edge_references()
            .map(|edge| [edge.source().index(), edge.target().index()])
            .collect();
        before.sort_unstable();
        let choices: Vec<_> = self
            .choices
            .iter()
            .map(|choice| {
                json!({"after":choice.after,"join":choice.join,"source":choice.source,
            "options":choice.options.iter().map(Route::evidence).collect::<Vec<_>>()})
            })
            .collect();
        json!({"source":route.source,"reference":route.reference,"nodes":nodes,"before":before,"choices":choices,
            "relations_complete":route.relations_complete,"peers_complete":route.peers_complete,"coverage":"unconfirmed"})
    }

    fn learn(
        source: Option<String>,
        candidate: &Candidate,
        candidates: &[Candidate],
        progression: &Progression,
    ) -> Result<(Self, Vec<Value>)> {
        if let Some(discovery) = choices::discover(&source, candidate, candidates, progression)? {
            return Ok(discovery);
        }
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
        let mut peers_complete = true;
        let correspondence = candidates
            .iter()
            .map(|peer| Ok((peer, peer.same_prefix(candidate, progression, progression)?)))
            .collect::<Result<Vec<_>>>()?;
        let complete: Vec<_> = correspondence
            .iter()
            .filter(|(peer, matches)| *matches && peer.nodes.len() == candidate.nodes.len())
            .map(|(peer, _)| *peer)
            .collect();
        for (peer, same_prefix) in &correspondence {
            let prefix = graph.node_count() > peer.nodes.len();
            // A faster complete peer can expose a due omission that the slower representative would excuse.
            let next_expected_ms = complete
                .iter()
                .filter_map(|reference| peer.next_expected_at(reference))
                .min();
            let censored = prefix && next_expected_ms.is_some_and(|at| peer.end_ms < at);
            let matches = *same_prefix && (!prefix || censored);
            let status = if matches && prefix {
                "censored"
            } else if matches {
                "matched"
            } else {
                "unresolved"
            };
            diagnostics.push(json!({"method":"representative-route-correspondence-v2","source":source,
                    "reference":candidate.reference,"trace":peer.reference,"status":status,
                    "scope_end_ms":peer.end_ms,"next_expected_ms":next_expected_ms,
                    "reason":if matches && prefix {"scope ended before the next representative occurrence"} else if matches {"same complete occurrence sequence"} else {"sequence differs or the next representative occurrence is due within the observed scope"}}));
            if !matches {
                peers_complete = false;
                continue;
            }
            for (node, (observation, _)) in graph.node_weights_mut().zip(&peer.nodes) {
                node.observations
                    .insert(peer.reference.clone(), *observation);
            }
        }
        let mut route = Self {
            source,
            reference: candidate.reference.clone(),
            graph,
            relations_complete: false,
            peers_complete,
            choices: Vec::new(),
        };
        route.refresh_relations(progression)?;
        Ok((route, diagnostics))
    }
}

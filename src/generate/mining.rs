use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use kdl::{KdlDocument, KdlEntry, KdlNode};
use process_mining::core::process_models::case_centric::dfg::DirectlyFollowsGraph;
use serde::{Deserialize, Serialize};

use crate::fflogs::model::CollectedLog;

mod alignment;
mod bosses;
mod candidates;
mod common;
mod occurrences;
mod progression;
mod repeats;
mod unclassified;
mod validation;
pub(super) use occurrences::history::Permutation;
pub(super) use occurrences::{
    Branch,
    Event as CommonEvent,
    HelperAssociation,
    HelperOccurrence,
    LoopDiscovery,
    LoopModel,
};
pub(super) use progression::{Draft, Progression};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) enum Identifier {
    Ability(i64),
    Marker(i64),
    Tether(i64),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(super) struct Signal {
    pub(super) kind: String,
    pub(super) identifier: Identifier,
    pub(super) source: Option<String>,
    pub(super) target: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct Batch {
    at_ms: i64,
    signals: Vec<Signal>,
    indices: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    unclassified_indices: Vec<usize>,
    #[serde(skip)]
    eligible_signals: Option<Vec<Signal>>,
}

impl Batch {
    fn eligible(&self) -> &[Signal] {
        self.eligible_signals.as_deref().unwrap_or(&self.signals)
    }

    // A display wave is one name/signal at one exact millisecond, e.g. eight simultaneous adds.
    fn waves(&self) -> std::collections::BTreeSet<&Signal> {
        self.signals.iter().collect()
    }
}

#[derive(Clone, Debug)]
struct Pull {
    source: String,
    source_language: String,
    hash: String,
    report: String,
    fight: i64,
    kill: bool,
    end_ms: i64,
    phase_transitions: Vec<super::PhaseTransition>,
    batches: Vec<Batch>,
    start_ms: i64,
    actors: BTreeMap<i64, String>,
    events: Arc<[crate::fflogs::model::Event]>,
    boss_names: std::collections::BTreeSet<String>,
    boss_ids: std::collections::BTreeSet<i64>,
    boss_scopes: BTreeMap<String, (i64, i64)>,
}

impl Pull {
    // Preserve raw batch positions, e.g. an unclassified wave between two anchors cannot shift indices.
    fn learning_view(&self) -> Cow<'_, Self> {
        if self
            .batches
            .iter()
            .all(|batch| batch.eligible_signals.is_none())
        {
            return Cow::Borrowed(self);
        }
        let mut view = self.clone();
        for batch in &mut view.batches {
            if let Some(signals) = batch.eligible_signals.take() {
                batch.signals = signals;
                batch
                    .indices
                    .retain(|index| !batch.unclassified_indices.contains(index));
                batch.unclassified_indices.clear();
            }
        }
        Cow::Owned(view)
    }

    fn scope_end(&self, source: &Option<String>) -> i64 {
        source
            .as_ref()
            .and_then(|source| self.boss_scopes.get(source))
            .map_or(self.end_ms, |(_, end)| *end)
    }
}

pub(super) struct Analysis {
    group: super::Group,
    pulls: Vec<Pull>,
    pub(super) ability_names: BTreeMap<i64, std::collections::BTreeSet<String>>,
    pub(super) boss_names: std::collections::BTreeSet<String>,
    graph: DirectlyFollowsGraph<'static>,
}

// Separate timestamps remain separate occurrences; entity multiplicity stays in the raw batch.
fn timing_table(pull: &Pull) -> BTreeMap<Signal, Vec<i64>> {
    let mut table = BTreeMap::<Signal, Vec<i64>>::new();
    for batch in &pull.batches {
        for signal in batch.waves() {
            table.entry(signal.clone()).or_default().push(batch.at_ms);
        }
    }
    table
}

impl Analysis {
    pub(super) fn history_models(&self) -> Result<Vec<occurrences::history::Permutation>> {
        let views: Vec<_> = self.pulls.iter().map(Pull::learning_view).collect();
        occurrences::history::discover(&views.iter().map(Cow::as_ref).collect::<Vec<_>>())
    }

    pub(super) fn progression(&self) -> Progression {
        Progression::new(&self.pulls)
    }

    pub(super) fn draft(&self, progression: &Progression, boss_only: bool) -> Result<Draft> {
        Draft::new(&self.pulls, progression, boss_only)
    }

    pub(super) fn needs_representative_route(&self) -> bool {
        self.pulls.len() == 1 || self.pulls.iter().filter(|pull| pull.kill).count() < 2
    }

    pub(super) fn draft_branches(
        &self,
        draft: &Draft,
        progression: &Progression,
    ) -> Result<Vec<Branch>> {
        draft.branches(&self.pulls, progression)
    }

    pub(super) fn draft_choice_validation(&self) -> Result<serde_json::Value> {
        Draft::validate_choices(&self.pulls)
    }

    pub(super) fn loop_models(&self, boss_only: bool) -> Result<LoopDiscovery> {
        let views: Vec<_> = self.pulls.iter().map(Pull::learning_view).collect();
        let mut discovery = occurrences::loop_models(
            &views.iter().map(Cow::as_ref).collect::<Vec<_>>(),
            boss_only,
        )?;
        occurrences::associate_helpers(&self.pulls.iter().collect::<Vec<_>>(), &mut discovery)?;
        Ok(discovery)
    }

    pub(super) fn aligned(
        &self,
        include: impl Fn(&Signal) -> bool,
        boss_only: bool,
    ) -> Result<occurrences::Common> {
        let classified: Vec<_> = self.pulls.iter().map(Pull::learning_view).collect();
        let selected: Vec<_> = classified.iter().map(Cow::as_ref).collect();
        let (views, diagnostics) = if boss_only {
            bosses::project(&selected)
        } else {
            (Vec::new(), Vec::new())
        };
        let pulls: Vec<_> = if boss_only {
            views.iter().collect()
        } else {
            selected
        };
        let common = Self::common(&pulls, &include)?;
        let mut result = occurrences::expand(&pulls, common, &include, !boss_only)?;
        result.diagnostics.extend(diagnostics);
        result
            .diagnostics
            .push(validation::check(&pulls, &include, boss_only)?);
        Ok(result)
    }

    pub(super) fn signals(&self) -> std::collections::BTreeSet<Signal> {
        self.pulls
            .iter()
            .flat_map(|pull| &pull.batches)
            .flat_map(|batch| batch.signals.iter().cloned())
            .collect()
    }

    pub(super) fn unclassified_signals(&self) -> std::collections::BTreeSet<Signal> {
        let eligible: std::collections::BTreeSet<_> = self
            .pulls
            .iter()
            .flat_map(|pull| &pull.batches)
            .flat_map(|batch| batch.eligible().iter())
            .collect();
        self.signals()
            .into_iter()
            .filter(|signal| !eligible.contains(signal))
            .collect()
    }

    pub(super) fn name(&self) -> Result<&str> {
        Ok(&self.group.pulls.first().context("Missing fight")?.name)
    }

    fn common(
        pulls: &[&Pull],
        include: impl Fn(&Signal) -> bool,
    ) -> Result<BTreeMap<Signal, BTreeMap<String, i64>>> {
        let completed: Vec<_> = pulls.iter().copied().filter(|pull| pull.kill).collect();
        ensure!(completed.len() >= 2, "Need at least two completed logs");
        let tables: Vec<_> = completed.iter().map(|pull| timing_table(pull)).collect();
        let mut common = common::anchors(&tables);
        common.retain(|signal, _| include(signal));
        Ok(common
            .into_iter()
            .map(|(signal, times)| {
                let samples = completed
                    .iter()
                    .zip(times)
                    .map(|(pull, at)| (format!("{}/{}", pull.report, pull.fight), at))
                    .collect();
                (signal, samples)
            })
            .collect())
    }

    pub(super) fn evidence(&self) -> serde_json::Value {
        let inputs: Vec<_> = self.pulls.iter().map(|pull| serde_json::json!({
            "file":pull.source,"sha256":pull.hash,"trace":format!("{}/{}",pull.report,pull.fight),
            "kill":pull.kill,"source_language":pull.source_language,"end_ms":pull.end_ms,
            "event_count":pull.events.len(),"phase_transitions":pull.phase_transitions,"observations":pull.batches,
        })).collect();
        serde_json::json!({"encounter":self.group.key.encounter,"difficulty":self.group.key.difficulty,"inputs":inputs,
            "boss_names":self.boss_names,"graph":{"activities":self.graph.activities.len(),"relations":self.graph.directly_follows_relations.len()}})
    }

    fn document(&self) -> Result<KdlDocument> {
        process_document(&self.group, &self.ability_names, &self.pulls, &self.graph)
    }
}

fn argument(node: &mut KdlNode, value: impl Into<kdl::KdlValue>) {
    node.push(KdlEntry::new(value));
}

fn prop(node: &mut KdlNode, name: &str, value: impl Into<kdl::KdlValue>) {
    node.push(KdlEntry::new_prop(name, value));
}

// Group equal-time packets so their order does not invent directly-follows edges.
fn batches(pull: &super::Pull, log: &CollectedLog) -> Result<Vec<Batch>> {
    let names: BTreeMap<_, _> = log
        .report
        .master_data
        .actors
        .iter()
        .map(|a| (a.id, a.name.as_str()))
        .collect();
    let mut groups = BTreeMap::<i64, (BTreeMap<_, (Signal, bool)>, Vec<usize>, Vec<usize>)>::new();
    let mut selected = Vec::new();
    for occurrence in pull.occurrences.iter() {
        let source = names
            .get(&occurrence.actor_id)
            .context("Missing event source name")?;
        // Listed enemies can also be anonymous; preserve them without inventing a runtime name.
        let unclassified = source.trim().is_empty();
        selected.push((
            occurrence.event_index,
            Signal {
                kind: occurrence.kind.clone(),
                identifier: Identifier::Ability(occurrence.ability_id),
                source: (!unclassified).then(|| (*source).to_owned()),
                target: None,
            },
            unclassified,
        ));
    }
    selected.extend(
        candidates::select(log)?
            .into_iter()
            .map(|(index, signal)| (index, signal, false)),
    );
    let known = selected.iter().map(|(index, _, _)| *index).collect();
    let extra = unclassified::casts(log, &known)?;
    for (index, signal, unclassified) in selected
        .into_iter()
        .chain(extra.into_iter().map(|(i, s)| (i, s, true)))
    {
        let event = log.events.get(index).context("Missing candidate event")?;
        let group = groups
            .entry(event.timestamp - log.collection.start_time as i64)
            .or_default();
        group.0.insert(
            (event.source_id, event.source_instance, signal.clone()),
            (signal, unclassified),
        );
        group.1.push(index);
        if unclassified {
            group.2.push(index);
        }
    }
    Ok(groups
        .into_iter()
        .map(
            |(at_ms, (signals, mut indices, mut unclassified_indices))| {
                let eligible_signals = (!unclassified_indices.is_empty()).then(|| {
                    let mut known: Vec<_> = signals
                        .values()
                        .filter(|(_, unclassified)| !unclassified)
                        .map(|(signal, _)| signal.clone())
                        .collect();
                    known.sort();
                    known
                });
                let mut signals: Vec<_> = signals.into_values().map(|(signal, _)| signal).collect();
                signals.sort();
                indices.sort_unstable();
                unclassified_indices.sort_unstable();
                Batch {
                    at_ms,
                    signals,
                    indices,
                    unclassified_indices,
                    eligible_signals,
                }
            },
        )
        .collect())
}

// Discover adjacency between batches without imposing an order inside a simultaneous batch.
fn discover(pulls: &[Pull]) -> Result<DirectlyFollowsGraph<'static>> {
    let mut graph = DirectlyFollowsGraph::new();
    for pull in pulls {
        discover_trace(&pull.batches, &mut graph)?;
    }
    Ok(graph)
}

// Reset adjacency per pull so the end of one fight never connects to the next fight.
fn discover_trace(batches: &[Batch], graph: &mut DirectlyFollowsGraph<'static>) -> Result<()> {
    let mut previous: Vec<String> = Vec::new();
    for batch in batches {
        let current = batch
            .signals
            .iter()
            .map(serde_json::to_string)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for key in &current {
            graph.add_activity(key.clone(), 1);
            for from in &previous {
                graph.add_df_relation(from.clone().into(), key.clone().into(), 1);
            }
        }
        previous = current;
    }
    Ok(())
}

// Give each signal its own identity, e.g. A stays the same activity in both A and A+B.
fn activity_node(
    id: &str,
    signal: &Signal,
    ability_names: &BTreeMap<i64, std::collections::BTreeSet<String>>,
    observations: i128,
    first: usize,
    last: usize,
) -> Result<KdlNode> {
    let mut activity = KdlNode::new("activity");
    argument(&mut activity, id);
    prop(&mut activity, "observations", observations);
    prop(&mut activity, "first_seen", i128::try_from(first)?);
    prop(&mut activity, "last_seen_in_kills", i128::try_from(last)?);
    let mut node = KdlNode::new("signal");
    argument(&mut node, signal.kind.as_str());
    let (field, id) = match signal.identifier {
        Identifier::Ability(id) => ("id", id),
        Identifier::Marker(id) => ("marker_id", id),
        Identifier::Tether(id) => ("tether_id", id),
    };
    prop(&mut node, field, format!("{id:X}"));
    for (field, name) in [("source", &signal.source), ("target", &signal.target)] {
        if let Some(name) = name {
            prop(&mut node, field, name.as_str());
        }
    }
    if let Identifier::Ability(id) = signal.identifier
        && let Some(names) = ability_names.get(&id)
        && names.len() == 1
        && let Some(name) = names.first()
    {
        prop(&mut node, "name", name.as_str());
    }
    activity.ensure_children().nodes_mut().push(node);

    Ok(activity)
}

// Preserve evidence independently of graph discovery, e.g. censored wipe traces.
fn trace_node(pull: &Pull, ids: &BTreeMap<String, String>) -> Result<KdlNode> {
    let mut trace = KdlNode::new("trace");
    argument(&mut trace, format!("{}/{}", pull.report, pull.fight));
    prop(&mut trace, "source", pull.source.as_str());
    prop(&mut trace, "source_language", pull.source_language.as_str());
    prop(&mut trace, "sha256", pull.hash.as_str());
    prop(&mut trace, "kill", pull.kill);
    prop(&mut trace, "end_ms", i128::from(pull.end_ms));
    for transition in &pull.phase_transitions {
        let mut boundary = KdlNode::new("phase-boundary");
        argument(&mut boundary, i128::from(transition.id));
        prop(&mut boundary, "at_ms", i128::from(transition.relative_ms));
        trace.ensure_children().nodes_mut().push(boundary);
    }
    for batch in &pull.batches {
        let mut observation = KdlNode::new("observed");
        for signal in &batch.signals {
            let key = serde_json::to_string(signal)?;
            argument(
                &mut observation,
                ids.get(&key).context("Missing trace activity")?.as_str(),
            );
        }
        prop(&mut observation, "at_ms", i128::from(batch.at_ms));
        for index in &batch.indices {
            argument(&mut observation, i128::try_from(*index)?);
        }
        if !batch.unclassified_indices.is_empty() {
            let mut unclassified = KdlNode::new("unclassified");
            for index in &batch.unclassified_indices {
                argument(&mut unclassified, i128::try_from(*index)?);
            }
            observation.ensure_children().nodes_mut().push(unclassified);
        }
        trace.ensure_children().nodes_mut().push(observation);
    }
    trace
        .ensure_children()
        .nodes_mut()
        .extend(repeats::nodes(&pull.batches, &pull.phase_transitions)?);
    // Keep all packets independently of cast selection, e.g. target-only markers and unknown fields.
    for (index, event) in pull.events.iter().enumerate() {
        let mut raw = KdlNode::new("raw-event");
        argument(&mut raw, i128::try_from(index)?);
        prop(
            &mut raw,
            "at_ms",
            i128::from(event.timestamp - pull.start_ms),
        );
        prop(&mut raw, "type", event.kind.as_str());
        for (field, actor) in [("source", event.source_id), ("target", event.target_id)] {
            if let Some(name) = actor.and_then(|id| pull.actors.get(&id)) {
                prop(&mut raw, field, name.as_str());
            }
        }
        prop(&mut raw, "data", serde_json::to_string(event)?);
        trace.ensure_children().nodes_mut().push(raw);
    }

    Ok(trace)
}

// Render the discovered graph deterministically before publishing a new output file.
fn process_document(
    group: &super::Group,
    ability_names: &BTreeMap<i64, std::collections::BTreeSet<String>>,
    pulls: &[Pull],
    graph: &DirectlyFollowsGraph<'_>,
) -> Result<KdlDocument> {
    let mut signals_by_key = BTreeMap::new();
    let mut starts = BTreeMap::<String, usize>::new();
    let mut ends = BTreeMap::<String, usize>::new();
    for pull in pulls {
        for batch in &pull.batches {
            for signal in &batch.signals {
                signals_by_key.insert(serde_json::to_string(signal)?, signal);
            }
        }
        if let Some(first) = pull.batches.first() {
            for signal in &first.signals {
                *starts.entry(serde_json::to_string(signal)?).or_default() += 1;
            }
        }
        if pull.kill
            && let Some(last) = pull.batches.last()
        {
            for signal in &last.signals {
                *ends.entry(serde_json::to_string(signal)?).or_default() += 1;
            }
        }
    }
    ensure!(
        pulls.iter().any(|pull| !pull.events.is_empty()),
        "No observations found"
    );
    let ids: BTreeMap<_, _> = signals_by_key
        .keys()
        .enumerate()
        .map(|(i, key)| (key.clone(), format!("a{}", i + 1)))
        .collect();
    let mut doc = KdlDocument::new();
    let mut root = KdlNode::new("process");
    argument(&mut root, 1_i128);
    prop(&mut root, "algorithm", "adjacent-batches");
    prop(
        &mut root,
        "name",
        group.pulls.first().context("Missing fight")?.name.as_str(),
    );
    prop(&mut root, "encounter", i128::from(group.key.encounter));
    prop(&mut root, "difficulty", i128::from(group.key.difficulty));
    for (key, signals) in &signals_by_key {
        let node = activity_node(
            ids.get(key).context("Missing activity ID")?,
            signals,
            ability_names,
            i128::from(
                *graph
                    .activities
                    .get(key)
                    .context("Missing activity frequency")?,
            ),
            *starts.get(key).unwrap_or(&0),
            *ends.get(key).unwrap_or(&0),
        )?;
        root.ensure_children().nodes_mut().push(node);
    }
    let mut edges: Vec<_> = graph.directly_follows_relations.iter().collect();
    edges.sort_by(|a, b| a.0.cmp(b.0));
    for ((from, to), count) in edges {
        let mut edge = KdlNode::new("follows");
        argument(
            &mut edge,
            ids.get(from.as_ref())
                .context("Missing source activity")?
                .as_str(),
        );
        argument(
            &mut edge,
            ids.get(to.as_ref())
                .context("Missing target activity")?
                .as_str(),
        );
        prop(&mut edge, "observations", i128::from(*count));
        root.ensure_children().nodes_mut().push(edge);
    }
    for pull in pulls {
        root.ensure_children()
            .nodes_mut()
            .push(trace_node(pull, &ids)?);
    }
    root.ensure_children()
        .nodes_mut()
        .push(common::node(pulls, &ids)?);
    doc.nodes_mut().push(root);
    doc.autoformat();
    Ok(doc)
}

fn body_ids(log: &CollectedLog, pull: &super::Pull) -> std::collections::BTreeSet<i64> {
    // Virtual helpers may be labeled Boss; only real NPCs establish body attempts.
    log.report
        .master_data
        .actors
        .iter()
        .filter(|actor| {
            actor.sub_type == "Boss"
                && pull.actors.contains_key(&actor.id)
                && !pull.virtual_actors.contains_key(&actor.id)
        })
        .map(|actor| actor.id)
        .collect()
}

pub(super) fn analyze(inputs: &[Utf8PathBuf]) -> Result<Analysis> {
    let hashes: BTreeMap<_, _> = inputs
        .iter()
        .map(|path| {
            let bytes = std::fs::read(path)?;
            Ok((
                super::slash_path(path.as_std_path()),
                ring::digest::digest(&ring::digest::SHA256, &bytes),
            ))
        })
        .collect::<Result<_>>()?;
    let mut groups = super::inspect(inputs)?;
    ensure!(
        groups.len() == 1,
        "Inputs must belong to one encounter and difficulty"
    );
    let group = groups.pop().context("Missing encounter")?;
    let mut pulls = Vec::new();
    let mut ability_names = BTreeMap::<i64, std::collections::BTreeSet<String>>::new();
    let mut boss_names = std::collections::BTreeSet::new();
    for pull in &group.pulls {
        let bytes = std::fs::read(&pull.file)?;
        let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
        ensure!(
            hashes
                .get(&pull.file)
                .is_some_and(|hash| hash.as_ref() == digest.as_ref()),
            "Input changed during analysis: {}",
            pull.file
        );
        let log: CollectedLog = serde_json::from_slice(&bytes)?;
        ensure!(
            log.report.code == pull.report
                && log
                    .report
                    .fights
                    .first()
                    .is_some_and(|fight| fight.id == pull.fight),
            "Input changed during analysis"
        );
        for ability in &log.report.master_data.abilities {
            if !ability.name.is_empty() {
                ability_names
                    .entry(ability.game_id)
                    .or_default()
                    .insert(ability.name.clone());
            }
        }
        let hash = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        let boss_ids = body_ids(&log, pull);
        let pull_bosses: std::collections::BTreeSet<_> = log
            .report
            .master_data
            .actors
            .iter()
            .filter(|actor| boss_ids.contains(&actor.id))
            .map(|actor| actor.name.clone())
            .collect();
        boss_names.extend(pull_bosses.iter().cloned());
        pulls.push(Pull {
            source: pull.file.clone(),
            source_language: log.report.master_data.lang.clone(),
            hash,
            report: pull.report.clone(),
            fight: pull.fight,
            kill: pull.kill,
            end_ms: pull.end_ms,
            phase_transitions: pull.phase_transitions.clone(),
            batches: batches(pull, &log)?,
            start_ms: log.collection.start_time as i64,
            actors: log
                .report
                .master_data
                .actors
                .iter()
                .map(|actor| (actor.id, actor.name.clone()))
                .collect(),
            events: log.events.into(),
            boss_names: pull_bosses,
            boss_ids,
            boss_scopes: BTreeMap::new(),
        });
    }
    let graph = discover(&pulls)?;
    Ok(Analysis {
        group,
        pulls,
        ability_names,
        boss_names,
        graph,
    })
}

pub(crate) fn mine(inputs: &[Utf8PathBuf], output: &Path) -> Result<()> {
    let doc = analyze(inputs)?.document()?;
    crate::output::write_new(&[(output, doc.to_string().as_bytes())])
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn batch(abilities: &[u8]) -> Batch {
        Batch {
            at_ms: 0,
            signals: abilities
                .iter()
                .map(|ability| Signal {
                    kind: "cast".into(),
                    identifier: Identifier::Ability(i64::from(*ability)),
                    source: Some("Boss".into()),
                    target: None,
                })
                .collect(),
            indices: Vec::new(),
            unclassified_indices: Vec::new(),
            eligible_signals: None,
        }
    }

    proptest! {
        #[test]
        fn wave_projection_ignores_entity_count_and_packet_order(
            ids in prop::collection::vec(0_u8..32,1..30), copies in 1_usize..10
        ) {
            let original = batch(&ids);
            let repeated:Vec<_> = ids.iter().rev().flat_map(|id|std::iter::repeat_n(*id,copies)).collect();
            let expanded = batch(&repeated);
            prop_assert_eq!(original.waves(),expanded.waves());
            prop_assert_eq!(expanded.signals.len(),ids.len()*copies);
        }

        #[test]
        fn simultaneous_packet_order_does_not_change_the_graph(
            groups in prop::collection::vec(prop::collection::vec(0_u8..8, 1..5), 1..8)
        ) {
            let batches: Vec<_> = groups.iter().map(|group| batch(group)).collect();
            let reversed: Vec<_> = groups.iter().map(|group| {
                let mut group = group.clone();
                group.reverse();
                batch(&group)
            }).collect();
            let mut first = DirectlyFollowsGraph::new();
            let mut second = DirectlyFollowsGraph::new();
            discover_trace(&batches, &mut first).unwrap();
            discover_trace(&reversed, &mut second).unwrap();
            prop_assert_eq!(first.activities, second.activities);
            prop_assert_eq!(first.directly_follows_relations, second.directly_follows_relations);
        }
    }

    #[test]
    fn independent_pulls_have_no_cross_fight_edges() {
        let mut graph = DirectlyFollowsGraph::new();
        discover_trace(&[batch(&[1])], &mut graph).unwrap();
        discover_trace(&[batch(&[2])], &mut graph).unwrap();
        assert_eq!(graph.activities.len(), 2);
        assert!(graph.directly_follows_relations.is_empty());
    }
}

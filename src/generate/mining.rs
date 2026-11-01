use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use kdl::{KdlDocument, KdlEntry, KdlNode};
use process_mining::core::event_data::case_centric::{Event, EventLog, Trace};
use process_mining::discovery::case_centric::dfg::discover_dfg;
use serde::{Deserialize, Serialize};

use crate::fflogs::model::CollectedLog;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
struct Signal {
    kind: String,
    ability: i64,
    source: String,
}

#[derive(Debug)]
struct Batch {
    at_ms: i64,
    signals: Vec<Signal>,
    indices: Vec<usize>,
}

#[derive(Debug)]
struct Pull {
    source: String,
    hash: String,
    report: String,
    fight: i64,
    kill: bool,
    end_ms: i64,
    phase_transitions: Vec<super::PhaseTransition>,
    batches: Vec<Batch>,
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
    let mut groups = BTreeMap::<i64, (BTreeMap<_, Signal>, Vec<usize>)>::new();
    for occurrence in pull.occurrences.iter() {
        let source = names
            .get(&occurrence.actor_id)
            .context("Missing event source name")?;
        ensure!(!source.is_empty(), "Empty event source name");
        let group = groups.entry(occurrence.relative_ms).or_default();
        group.0.insert(
            (
                occurrence.actor_id,
                occurrence.instance,
                occurrence.kind.clone(),
                occurrence.ability_id,
            ),
            Signal {
                kind: occurrence.kind.clone(),
                ability: occurrence.ability_id,
                source: (*source).to_owned(),
            },
        );
        group.1.push(occurrence.event_index);
    }
    Ok(groups
        .into_iter()
        .map(|(at_ms, (signals, indices))| {
            let mut signals: Vec<_> = signals.into_values().collect();
            signals.sort();
            Batch {
                at_ms,
                signals,
                indices,
            }
        })
        .collect())
}

// Serialize signal multiplicity without inventing separate graph activities.
fn activity_node(
    id: &str,
    signals: &[Signal],
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
    let mut counts = BTreeMap::<&Signal, usize>::new();
    for signal in signals {
        *counts.entry(signal).or_default() += 1;
    }
    for (signal, count) in counts {
        let mut node = KdlNode::new("signal");
        argument(&mut node, signal.kind.as_str());
        prop(&mut node, "id", format!("{:X}", signal.ability));
        prop(&mut node, "source", signal.source.as_str());
        if let Some(names) = ability_names.get(&signal.ability)
            && names.len() == 1
            && let Some(name) = names.first()
        {
            prop(&mut node, "name", name.as_str());
        }
        if count > 1 {
            prop(&mut node, "count", i128::try_from(count)?);
        }
        activity.ensure_children().nodes_mut().push(node);
    }

    Ok(activity)
}

// Preserve evidence independently of graph discovery, e.g. censored wipe traces.
fn trace_node(pull: &Pull, ids: &BTreeMap<String, String>) -> Result<KdlNode> {
    let mut trace = KdlNode::new("trace");
    argument(&mut trace, format!("{}/{}", pull.report, pull.fight));
    prop(&mut trace, "source", pull.source.as_str());
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
        let key = serde_json::to_string(&batch.signals)?;
        let mut observation = KdlNode::new("observed");
        argument(
            &mut observation,
            ids.get(&key).context("Missing trace activity")?.as_str(),
        );
        prop(&mut observation, "at_ms", i128::from(batch.at_ms));
        for index in &batch.indices {
            argument(&mut observation, i128::try_from(*index)?);
        }
        trace.ensure_children().nodes_mut().push(observation);
    }

    Ok(trace)
}

// Render the discovered graph deterministically before publishing a new output file.
fn process_document(
    group: &super::Group,
    language: Option<&str>,
    ability_names: &BTreeMap<i64, std::collections::BTreeSet<String>>,
    pulls: &[Pull],
) -> Result<KdlDocument> {
    let mut log = EventLog::new();
    let mut signals_by_key = BTreeMap::new();
    let mut starts = BTreeMap::<String, usize>::new();
    let mut ends = BTreeMap::<String, usize>::new();
    for pull in pulls {
        let mut trace = Trace::new();
        for batch in &pull.batches {
            let key = serde_json::to_string(&batch.signals)?;
            signals_by_key.insert(key.clone(), batch.signals.clone());
            trace.events.push(Event::new(key));
        }
        if let Some(first) = pull.batches.first() {
            *starts
                .entry(serde_json::to_string(&first.signals)?)
                .or_default() += 1;
        }
        if pull.kill
            && let Some(last) = pull.batches.last()
        {
            *ends
                .entry(serde_json::to_string(&last.signals)?)
                .or_default() += 1;
        }
        log.traces.push(trace);
    }
    ensure!(
        !signals_by_key.is_empty(),
        "No enemy cast observations found"
    );
    let graph = discover_dfg(&log);
    let ids: BTreeMap<_, _> = signals_by_key
        .keys()
        .enumerate()
        .map(|(i, key)| (key.clone(), format!("a{}", i + 1)))
        .collect();
    let mut doc = KdlDocument::new();
    let mut root = KdlNode::new("process");
    argument(&mut root, 1_i128);
    prop(&mut root, "algorithm", "directly-follows");
    prop(&mut root, "encounter", i128::from(group.key.encounter));
    prop(&mut root, "difficulty", i128::from(group.key.difficulty));
    prop(&mut root, "language", language.unwrap_or(""));
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
    doc.nodes_mut().push(root);
    doc.autoformat();
    Ok(doc)
}

pub(crate) fn mine(inputs: &[Utf8PathBuf], output: &Path) -> Result<()> {
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
    let groups = super::inspect(inputs)?;
    let [group] = groups.as_slice() else {
        anyhow::bail!("Inputs must belong to one encounter and difficulty")
    };
    let mut pulls = Vec::new();
    let mut language = None;
    let mut ability_names = BTreeMap::<i64, std::collections::BTreeSet<String>>::new();
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
        let lang = &log.report.master_data.lang;
        ensure!(
            language.as_ref().is_none_or(|previous| previous == lang),
            "Inputs use different languages"
        );
        language = Some(lang.clone());
        for ability in &log.report.master_data.abilities {
            if !ability.name.is_empty() {
                ability_names
                    .entry(ability.game_id)
                    .or_default()
                    .insert(ability.name.clone());
            }
        }
        let hash = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        pulls.push(Pull {
            source: pull.file.clone(),
            hash,
            report: pull.report.clone(),
            fight: pull.fight,
            kill: pull.kill,
            end_ms: pull.end_ms,
            phase_transitions: pull.phase_transitions.clone(),
            batches: batches(pull, &log)?,
        });
    }
    let doc = process_document(group, language.as_deref(), &ability_names, &pulls)?;
    crate::output::write_new(&[(output, doc.to_string().as_bytes())])
}

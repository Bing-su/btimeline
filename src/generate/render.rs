use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use kdl::{KdlDocument, KdlNode, KdlValue};
use serde_json::{Value, json};

fn children(node: &KdlNode) -> &[KdlNode] {
    node.children().map_or(&[], |document| document.nodes())
}

fn text(node: &KdlNode, field: &str) -> Result<String> {
    let value = node
        .get(field)
        .and_then(KdlValue::as_string)
        .context(format!("Missing {field}"))?;
    ensure!(!value.trim().is_empty(), "Empty {field}");
    Ok(value.to_owned())
}

fn id(node: &KdlNode) -> Result<String> {
    Ok(node
        .get(0)
        .and_then(KdlValue::as_string)
        .context("Missing ID")?
        .to_owned())
}

fn integer(node: &KdlNode, field: &str) -> Result<i64> {
    i64::try_from(
        node.get(field)
            .and_then(KdlValue::as_integer)
            .context(format!("Missing {field}"))?,
    )
    .context("Integer out of range")
}

fn one<'a>(nodes: &'a [KdlNode], name: &str) -> Result<&'a KdlNode> {
    let mut matching = nodes.iter().filter(|node| node.name().value() == name);
    let node = matching.next().context(format!("Missing {name}"))?;
    ensure!(matching.next().is_none(), "Duplicate {name}");
    Ok(node)
}

fn template(source: &str) -> Result<(KdlDocument, String, String)> {
    crate::timeline::parse(source).context("Invalid timeline template")?;
    let doc: KdlDocument = source.parse()?;
    let root = doc.nodes().first().context("Missing timeline")?;
    ensure!(
        children(root)
            .iter()
            .any(|node| node.name().value() == "start"),
        "Template needs a start signal"
    );
    ensure!(
        children(root)
            .iter()
            .any(|node| matches!(node.name().value(), "wipe" | "finish" | "zone-exit")),
        "Template needs a reset signal: wipe, finish, or zone-exit"
    );
    let anchor = one(children(root), "event")?;
    ensure!(
        anchor
            .get(1)
            .is_some_and(|value| value.as_integer() == Some(0)
                || value.as_float().is_some_and(|value| value.abs() < 1e-7)),
        "Template event must be at 0.0"
    );
    ensure!(
        children(anchor).is_empty(),
        "Template origin event cannot have syncs yet"
    );
    for node in children(root) {
        ensure!(
            matches!(
                node.name().value(),
                "event" | "start" | "wipe" | "finish" | "zone-exit"
            ),
            "Template supports lifecycle and one root origin event only"
        );
        if node.name().value() != "event" {
            ensure!(
                !id(node)?.starts_with("FFLogs."),
                "Template requires runtime signals, not FFLogs-only signals"
            );
        }
    }
    let name = text(anchor, "name")?;
    let anchor_id = id(anchor)?;
    Ok((doc, name, anchor_id))
}

#[derive(Debug)]
struct Anchor {
    activity: String,
    name: String,
    source: String,
    ability: String,
    samples: BTreeMap<String, i64>,
}

fn read_samples(
    node: &KdlNode,
    completed: &BTreeMap<String, i64>,
) -> Result<BTreeMap<String, i64>> {
    let mut samples = BTreeMap::new();
    for sample in children(node) {
        ensure!(sample.name().value() == "sample", "Unknown anchor child");
        let trace = id(sample)?;
        let at = integer(sample, "at_ms")?;
        ensure!(
            completed
                .get(&trace)
                .is_some_and(|end| 0 <= at && at <= *end),
            "Sample outside a completed trace"
        );
        ensure!(
            samples.insert(trace, at).is_none(),
            "Duplicate anchor sample"
        );
    }
    ensure!(
        samples.len() == completed.len(),
        "Anchor must have one sample per completed trace"
    );
    Ok(samples)
}

// Reject ambiguous edits, e.g. duplicate status properties must not silently pick the last one.
fn validate_graph_nodes(root: &KdlNode) -> Result<()> {
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        ensure!(
            node.ty().is_none(),
            "Graph type annotations are unsupported"
        );
        let mut properties = BTreeSet::new();
        for entry in node.entries() {
            ensure!(
                entry.ty().is_none(),
                "Graph entry annotations are unsupported"
            );
            if let Some(name) = entry.name() {
                ensure!(
                    properties.insert(name.value()),
                    "Duplicate graph property {}",
                    name.value()
                );
            }
        }
        pending.extend(children(node));
    }
    Ok(())
}

fn graph(source: &str) -> Result<(Vec<Anchor>, Vec<Value>)> {
    let doc: KdlDocument = source.parse()?;
    let [root] = doc.nodes() else {
        anyhow::bail!("Expected one process graph");
    };
    validate_graph_nodes(root)?;
    ensure!(
        root.name().value() == "process" && root.get(0).and_then(KdlValue::as_integer) == Some(1),
        "Unsupported process graph"
    );
    ensure!(
        text(root, "algorithm")? == "adjacent-batches",
        "Unsupported graph algorithm"
    );
    let nodes = children(root);
    let common = one(nodes, "common-progress")?;
    ensure!(
        text(common, "status")? == "observed-common-order",
        "Generation requires an agreed common order"
    );
    let mut completed = BTreeMap::new();
    for trace in nodes.iter().filter(|node| node.name().value() == "trace") {
        if trace.get("kill").and_then(KdlValue::as_bool) == Some(true) {
            ensure!(
                completed
                    .insert(id(trace)?, integer(trace, "end_ms")?)
                    .is_none(),
                "Duplicate completed trace"
            );
        }
    }
    ensure!(
        completed.len() >= 2
            && integer(common, "completed_pulls")? == i64::try_from(completed.len())?,
        "Need at least two completed traces"
    );
    let mut activities = BTreeMap::new();
    for activity in nodes
        .iter()
        .filter(|node| node.name().value() == "activity")
    {
        ensure!(
            activities
                .insert(id(activity)?, one(children(activity), "signal")?)
                .is_none(),
            "Duplicate activity"
        );
    }
    let mut anchors = Vec::new();
    let mut excluded = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in children(common)
        .iter()
        .filter(|node| node.name().value() == "anchor")
    {
        let activity = id(anchor)?;
        ensure!(seen.insert(activity.clone()), "Duplicate common anchor");
        let signal = activities
            .get(&activity)
            .context("Unknown common activity")?;
        let samples = read_samples(anchor, &completed)?;
        if id(signal)? != "cast" {
            excluded.push(json!({"activity":activity,"reason":"non-cast anchor"}));
            continue;
        }
        let ability = text(signal, "id")?;
        ensure!(
            ability.bytes().all(|byte| byte.is_ascii_hexdigit())
                && i64::from_str_radix(&ability, 16).is_ok_and(|id| id >= 0),
            "Invalid ability ID"
        );
        anchors.push(Anchor {
            activity,
            name: text(signal, "name")?,
            source: text(signal, "source")?,
            ability,
            samples,
        });
    }
    for activity in activities
        .keys()
        .filter(|activity| !seen.contains(*activity))
    {
        excluded.push(json!({"activity":activity,"reason":"not a unique common anchor"}));
    }
    Ok((anchors, excluded))
}

fn slug(name: &str) -> String {
    name.split(|character: char| !character.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(serde::Serialize)]
struct Sample<'a> {
    trace: &'a str,
    at_ms: i64,
    origin_ms: i64,
    offset_ms: i64,
}

fn schedule(
    anchors: &[Anchor],
    origin: &Anchor,
    used_id: &str,
) -> Result<(Vec<KdlNode>, Vec<Value>, Vec<Value>)> {
    let mut events = Vec::new();
    let mut evidence = Vec::new();
    let mut excluded = Vec::new();
    let mut used = BTreeSet::from([used_id.to_owned()]);
    for anchor in anchors
        .iter()
        .filter(|anchor| anchor.activity != origin.activity)
    {
        let samples = anchor
            .samples
            .iter()
            .map(|(trace, at)| {
                let zero = origin.samples.get(trace).context("Missing origin sample")?;
                Ok(Sample {
                    trace,
                    at_ms: *at,
                    origin_ms: *zero,
                    offset_ms: at - zero,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut offsets: Vec<_> = samples.iter().map(|sample| sample.offset_ms).collect();
        if offsets.iter().any(|offset| *offset < 0) {
            excluded.push(json!({"activity":anchor.activity,"reason":"before authored origin in a completed trace","samples":samples}));
            continue;
        }
        offsets.sort_unstable();
        let median = *offsets
            .get((offsets.len() - 1) / 2)
            .context("Missing schedule samples")?;
        ensure!(
            median <= crate::timeline::MAX_MS,
            "Schedule exceeds supported time range"
        );
        let tick = (median + 50) / 100;
        let event_id = format!(
            "{}-{}-{}",
            slug(&anchor.name),
            slug(&anchor.source),
            anchor.ability.to_lowercase()
        );
        ensure!(
            used.insert(event_id.clone()),
            "Generated event ID collision: {event_id}"
        );
        let mut event = KdlNode::new("event");
        event.push(event_id.clone());
        event.push(tick as f64 / 10.0);
        event.push(kdl::KdlEntry::new_prop("name", anchor.name.clone()));
        events.push((tick, event_id.clone(), event));
        evidence.push(
            json!({"event":event_id,"activity":anchor.activity,"median_offset_ms":median,
            "rounded_offset_ms":tick*100,"rounding_delta_ms":tick*100-median,"samples":samples}),
        );
    }
    events.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    Ok((
        events.into_iter().map(|(_, _, event)| event).collect(),
        evidence,
        excluded,
    ))
}

fn hash(source: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, source.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// Keep the authored clock contract intact; FFLogs fight boundaries never manufacture lifecycle signals.
pub(crate) fn generate(graph_path: &Path, template_path: &Path, output: &Path) -> Result<usize> {
    let graph_source = std::fs::read_to_string(graph_path)?;
    let template_source = std::fs::read_to_string(template_path)?;
    let (mut doc, origin_name, origin_id) = template(&template_source)?;
    let (anchors, mut excluded) = graph(&graph_source)?;
    let mut origins = anchors.iter().filter(|anchor| anchor.name == origin_name);
    let origin = origins
        .next()
        .context("Origin event name must match a unique common cast name")?;
    ensure!(
        origins.next().is_none(),
        "Origin cast name matches multiple sources or abilities"
    );
    let (events, evidence, skipped) = schedule(&anchors, origin, &origin_id)?;
    ensure!(
        !events.is_empty(),
        "No common cast events after the authored origin"
    );
    let count = events.len() + 1;
    excluded.extend(skipped);
    doc.nodes_mut()
        .first_mut()
        .context("Missing timeline")?
        .ensure_children()
        .nodes_mut()
        .extend(events);
    doc.autoformat();
    let timeline = doc.to_string();
    crate::timeline::parse(&timeline).context("Generated timeline failed validation")?;
    let report = json!({"version":1,"graph":{"path":graph_path,"sha256":hash(&graph_source)},
        "template":{"path":template_path,"sha256":hash(&template_source)},
        "origin":{"event":origin_id,"activity":origin.activity,"name":origin.name,"samples":origin.samples,
            "binding":"author asserts that lifecycle start and the named FFLogs cast share time zero"},
        "timeline_sha256":hash(&timeline),"events":evidence,"excluded":excluded,
        "not_checked":["origin signal equivalence","English name accuracy","game accuracy","missing-signal coverage","branches and loops"]});
    crate::output::write_new(&[
        (output, timeline.as_bytes()),
        (
            &output.with_extension("evidence.json"),
            &serde_json::to_vec_pretty(&report)?,
        ),
    ])?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case("start \"Ability\" id=\"A001\"")]
    #[case("wipe \"ActorControl\" command=\"4000000F\"")]
    fn generation_requires_an_explicit_start_and_reset(#[case] lifecycle: &str) {
        let source = format!(
            "timeline 1 \"Example\" {{\n {lifecycle}\n event \"origin\" 0.0 name=\"A\"\n}}\n"
        );
        let error = template(&source).unwrap_err();
        assert!(error.to_string().contains("Template needs"));
    }

    #[test]
    fn edited_graph_properties_cannot_hide_earlier_values() {
        let mut node = KdlNode::new("common-progress");
        node.push(kdl::KdlEntry::new_prop("status", "ambiguous-order"));
        node.push(kdl::KdlEntry::new_prop("status", "observed-common-order"));
        let error = validate_graph_nodes(&node).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Duplicate graph property status")
        );
    }
}

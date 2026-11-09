use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use serde_json::{Value, json};

use super::mining::{
    Analysis,
    Branch,
    CommonEvent,
    Draft,
    Identifier,
    LoopDiscovery,
    LoopModel,
    Progression,
    Signal,
};
use super::timing::{self, median};

mod branches;
mod history;
mod loops;

#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Raid,
    Dungeon,
    Alliance,
}

impl FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "raid" => Ok(Self::Raid),
            "dungeon" => Ok(Self::Dungeon),
            "alliance" => Ok(Self::Alliance),
            _ => anyhow::bail!("Unknown mode {value}; use raid, dungeon, or alliance"),
        }
    }
}

#[derive(Clone)]
struct Anchor {
    activity: Signal,
    name: String,
    source: String,
    ability: String,
    observations: BTreeMap<String, usize>,
    occurrence: Option<usize>,
    alignment: Value,
}

struct Prepared {
    events: Vec<Anchor>,
    branches: Vec<Branch>,
    excluded: Vec<Value>,
    diagnostics: Vec<Value>,
}

fn anchor(analysis: &Analysis, progression: &Progression, row: &CommonEvent) -> Result<Anchor> {
    let activity = row.signal.clone();
    let Identifier::Ability(ability) = activity.identifier else {
        anyhow::bail!("Cast requires an ability ID")
    };
    ensure!(ability >= 0, "Cast requires a nonnegative ability ID");
    let names = analysis
        .ability_names
        .get(&ability)
        .context("Missing cast name")?;
    ensure!(
        names.len() == 1,
        "Ambiguous cast name for ability {ability:X}"
    );
    let name = names.first().context("Missing cast name")?.clone();
    let source = activity
        .source
        .clone()
        .filter(|name| !name.trim().is_empty())
        .context("Missing caster name")?;
    Ok(Anchor {
        activity,
        name,
        source,
        ability: format!("{ability:X}"),
        observations: progression.bind(&row.signal, &row.samples)?,
        occurrence: row.occurrence,
        alignment: row.evidence.clone(),
    })
}

fn draft_anchors(
    analysis: &Analysis,
    progression: &Progression,
    draft: &Draft,
) -> Result<Prepared> {
    let branches = analysis.draft_branches(draft, progression)?;
    let diagnostics = Vec::new();
    let mut events = Vec::new();
    for route in &draft.routes {
        let mut counts = BTreeMap::<_, usize>::new();
        for node in route.graph.node_weights() {
            *counts.entry(&node.activity).or_default() += 1;
        }
        for node in route.graph.node_weights() {
            events.push(anchor(analysis, progression, &CommonEvent {
                signal: node.activity.clone(),
                samples: progression.samples(&node.observations)?,
                occurrence: counts.get(&node.activity).filter(|count| **count > 1).map(|_| node.occurrence),
                evidence: json!({"method":"representative-route-v1","reference":route.reference,"source":route.source,
                    "coverage":"unconfirmed"}),
            })?);
        }
    }
    ensure!(
        !events.is_empty(),
        "No eligible casts in a supported representative scope"
    );
    let seen: BTreeSet<_> = events
        .iter()
        .map(|event| &event.activity)
        .chain(
            branches
                .iter()
                .flat_map(|branch| &branch.options)
                .flat_map(|option| option.events.iter().map(|event| &event.signal)),
        )
        .collect();
    let excluded = analysis.signals().iter().filter(|activity| !seen.contains(activity))
        .map(|activity| json!({"activity":activity,"reason":"outside representative scope or not execution eligible"})).collect();
    Ok(Prepared {
        events,
        branches,
        excluded,
        diagnostics,
    })
}

// Consume the shared in-memory mining result; generation never requires a serialized graph input.
fn anchors(analysis: &Analysis, progression: &Progression, mode: Mode) -> Result<Prepared> {
    let include = |signal: &Signal| {
        signal.kind == "cast"
            && (matches!(mode, Mode::Raid)
                || signal
                    .source
                    .as_ref()
                    .is_some_and(|source| analysis.boss_names.contains(source)))
    };
    if !matches!(mode, Mode::Raid) {
        ensure!(
            !analysis.boss_names.is_empty(),
            "Input metadata does not identify Boss actors"
        );
    }
    let common = analysis.aligned(include, !matches!(mode, Mode::Raid))?;
    let events = common
        .events
        .iter()
        .map(|row| anchor(analysis, progression, row))
        .collect::<Result<Vec<_>>>()?;
    let seen: BTreeSet<_> = common
        .events
        .iter()
        .map(|row| row.signal.clone())
        .chain(
            common
                .branches
                .iter()
                .flat_map(|branch| &branch.options)
                .flat_map(|option| option.events.iter().map(|event| event.signal.clone())),
        )
        .collect();
    let mut excluded = Vec::new();
    let unclassified = analysis.unclassified_signals();
    for activity in analysis.signals().difference(&seen) {
        let reason = if unclassified.contains(activity) {
            "unclassified NPC cast; no inferred boss ownership or execution eligibility"
        } else if activity.kind != "cast" {
            "non-cast activity"
        } else if !include(activity) {
            "not a Boss actor"
        } else {
            "no forced common occurrence or supported branch"
        };
        excluded.push(json!({"activity":activity,"reason":reason}));
    }
    ensure!(
        !events.is_empty(),
        "No common casts with a supported shared order"
    );
    Ok(Prepared {
        events,
        branches: common.branches,
        excluded,
        diagnostics: common.diagnostics,
    })
}

fn prop(node: &mut KdlNode, name: &str, value: impl Into<KdlValue>) {
    node.push(KdlEntry::new_prop(name, value));
}
fn signal(role: &str, kind: &str, fields: &[(&str, &str)]) -> KdlNode {
    let mut node = KdlNode::new(role);
    node.push(kind);
    for (name, value) in fields {
        prop(&mut node, name, *value);
    }
    node
}

// Cactbot uses InCombat for raid starts and explicit action-ID/NPC-name anchors for encounter sync.
// Local boss anchoring avoids starting a dungeon/alliance boss schedule on preceding trash combat.
fn starts(mode: Mode, origins: &[&Anchor]) -> Vec<KdlNode> {
    match mode {
        Mode::Raid => vec![signal("start", "InCombat", &[("inGameCombat", "1")])],
        _ => origins
            .iter()
            .map(|origin| {
                signal(
                    "start",
                    "Ability",
                    &[("id", &origin.ability), ("source", &origin.source)],
                )
            })
            .collect(),
    }
}

fn clocks<'a>(
    anchors: &'a [Anchor],
    progression: &Progression,
    mode: Mode,
) -> Result<(BTreeMap<String, i64>, Vec<&'a Anchor>)> {
    ensure!(!anchors.is_empty(), "Missing timing samples");
    let samples = anchors
        .iter()
        .map(|anchor| progression.samples(&anchor.observations))
        .collect::<Result<Vec<_>>>()?;
    let mut zeros = BTreeMap::new();
    for trace in samples
        .iter()
        .flat_map(BTreeMap::keys)
        .collect::<BTreeSet<_>>()
    {
        let zero = match mode {
            Mode::Raid => 0,
            _ => *samples
                .iter()
                .filter_map(|samples| samples.get(trace))
                .min()
                .context("Missing boss origin")?,
        };
        zeros.insert(trace.clone(), zero);
    }
    let origins = match mode {
        Mode::Raid => Vec::new(),
        _ => anchors
            .iter()
            .zip(&samples)
            .filter(|(_, samples)| {
                samples
                    .iter()
                    .any(|(trace, at)| zeros.get(trace) == Some(at))
            })
            .map(|(anchor, _)| anchor)
            .collect(),
    };
    Ok((zeros, origins))
}

fn slug(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("-")
}

fn event_id(anchor: &Anchor, context: Option<&str>, disambiguate: bool) -> String {
    let mut id = format!(
        "{}-{}-{}",
        slug(&anchor.name),
        slug(&anchor.source),
        anchor.ability.to_lowercase()
    );
    if let Some(occurrence) = anchor.occurrence {
        id.push_str(&format!("-{occurrence:02}"));
    }
    // Local references can reuse an ordinal, e.g. B-03 before C and another B-03 after C.
    if disambiguate {
        for (label, pointer) in [
            ("after", "/bounds/left/identifier/Ability"),
            ("before", "/bounds/right/identifier/Ability"),
        ] {
            if let Some(ability) = anchor.alignment.pointer(pointer).and_then(Value::as_i64) {
                id.push_str(&format!("-{label}-{ability:x}"));
            }
        }
    }
    if let Some(context) = context {
        id = format!("{context}-{id}");
    }
    id
}

fn schedule(
    anchors: &[Anchor],
    zeros: &BTreeMap<String, i64>,
    mode: Mode,
    context: Option<&str>,
    progression: &Progression,
) -> Result<(Vec<KdlNode>, Vec<Value>)> {
    let mut events = Vec::new();
    let mut evidence = Vec::new();
    let mut used = BTreeSet::new();
    let mut label_counts = BTreeMap::<_, usize>::new();
    for anchor in anchors {
        *label_counts
            .entry((&anchor.activity, anchor.occurrence))
            .or_default() += 1;
    }
    for anchor in anchors {
        let timing::Estimate {
            samples,
            median_ms: time,
            tick,
            min_tick,
            max_tick,
        } = timing::estimate(&progression.samples(&anchor.observations)?, zeros)?;
        let seconds = tick as f64 / 10.0;
        let event_id = event_id(
            anchor,
            context,
            label_counts
                .get(&(&anchor.activity, anchor.occurrence))
                .is_some_and(|count| *count > 1),
        );
        ensure!(
            used.insert(event_id.clone()),
            "Generated event ID collision: {event_id}"
        );
        let mut event = KdlNode::new("event");
        event.push(event_id.clone());
        event.push(seconds);
        prop(&mut event, "name", anchor.name.as_str());
        let mut sync = KdlNode::new("sync");
        sync.push(seconds);
        sync.push("Ability");
        prop(&mut sync, "id", anchor.ability.as_str());
        prop(&mut sync, "source", anchor.source.as_str());
        prop(&mut sync, "before", (tick - min_tick).max(25) as f64 / 10.0);
        prop(&mut sync, "after", (max_tick - tick).max(25) as f64 / 10.0);
        // Repeated identities can match an earlier cast; only unique anchors adjust the clock.
        if anchor.occurrence.is_none() {
            event.ensure_children().nodes_mut().push(sync);
        }
        events.push((tick, event_id.clone(), event));
        evidence.push(
            json!({"event":event_id,"activity":anchor.activity,"median_offset_ms":time,
            "rounded_offset_ms":tick*100,"rounding_delta_ms":tick*100-time,"samples":samples,
            "alignment":anchor.alignment,"observation_nodes":anchor.observations}),
        );
    }
    events.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    // Cactbot opening anchors use wider windows to correct the initial combat-clock estimate.
    if matches!(mode, Mode::Raid)
        && let Some((_, _, event)) = events.iter_mut().find(|(_, _, event)| {
            event
                .children()
                .is_some_and(|body| !body.nodes().is_empty())
        })
    {
        let sync = event
            .ensure_children()
            .nodes_mut()
            .first_mut()
            .context("Missing opening sync")?;
        for field in ["before", "after"] {
            let value = sync.get_mut(field).context("Missing sync window")?;
            *value = KdlValue::Float(value.as_float().context("Invalid sync window")?.max(10.0));
        }
    }
    Ok((
        events.into_iter().map(|(_, _, event)| event).collect(),
        evidence,
    ))
}

#[derive(Default)]
struct Rendered {
    nodes: Vec<KdlNode>,
    events: Vec<Value>,
    origins: Vec<Value>,
    loops: Vec<Value>,
    histories: Vec<Value>,
}

impl Rendered {
    fn event_count(&self) -> usize {
        self.events.len()
            + self
                .loops
                .iter()
                .filter_map(|item| item["events"].as_array())
                .map(Vec::len)
                .sum::<usize>()
    }
}

// Resolve evidence references from the same authored ownership used by the compiler.
fn collect_references(
    node: &KdlNode,
    owner: &str,
    references: &mut BTreeMap<String, String>,
) -> Result<()> {
    let role = node.name().value();
    let local = node.get(0).and_then(KdlValue::as_string);
    let path = match (role, local) {
        ("event" | "branch" | "option", Some(id)) => format!("{owner}/{id}"),
        ("loop", Some(id)) => format!("{owner}/loop/{id}"),
        ("merge", _) => format!("{owner}/merge"),
        _ => owner.to_owned(),
    };
    if matches!(role, "event" | "branch" | "loop")
        && let Some(id) = local
    {
        ensure!(
            references.insert(id.to_owned(), path.clone()).is_none(),
            "Generated reference collision: {id}"
        );
    }
    for child in node.children().into_iter().flat_map(|body| body.nodes()) {
        collect_references(child, &path, references)?;
    }
    Ok(())
}

fn evidence_references(value: &mut Value, references: &BTreeMap<String, String>) {
    match value {
        Value::Object(fields) => {
            for key in ["event", "phase", "branch"] {
                if let Some(current) = fields.get(key).and_then(Value::as_str)
                    && let Some(reference) = references.get(current)
                {
                    fields.insert(key.into(), reference.clone().into());
                }
            }
            if let Some(events) = fields.get_mut("events") {
                evidence_references(events, references);
            }
        }
        Value::Array(rows) => {
            for row in rows {
                evidence_references(row, references);
            }
        }
        _ => {}
    }
}

fn group_steps(rendered: &mut Rendered) -> Result<()> {
    let mut references = BTreeMap::new();
    for node in &mut rendered.nodes {
        if node.name().value() != "step" {
            continue;
        }
        let id = node
            .get(0)
            .and_then(KdlValue::as_string)
            .context("Missing generated step ID")?
            .to_owned();
        let path = format!("{id}/main");
        references.insert(id.clone(), path.clone());
        for child in node.children().into_iter().flat_map(|body| body.nodes()) {
            collect_references(child, &path, &mut references)?;
        }
        let mut step = node.clone();
        *step.get_mut(0).context("Missing generated step ID")? = KdlValue::String("main".into());
        let mut phase = KdlNode::new("phase");
        phase.push(id);
        phase.ensure_children().nodes_mut().push(step);
        *node = phase;
    }
    for row in rendered.events.iter_mut().chain(&mut rendered.origins) {
        evidence_references(row, &references);
    }
    for row in &mut rendered.loops {
        if let Some(id) = row["id"].as_str().and_then(|id| references.get(id)) {
            row["id"] = id.clone().into();
        }
        evidence_references(row, &references);
    }
    Ok(())
}

// Boss metadata supplies scope boundaries; travel and preceding trash never share a boss clock.
fn compose(
    analysis: &Analysis,
    progression: &Progression,
    anchors: Vec<Anchor>,
    choices: Vec<Branch>,
    mode: Mode,
    models: &[LoopModel],
) -> Result<Rendered> {
    let mut scopes = BTreeMap::<Option<String>, Vec<Anchor>>::new();
    for anchor in anchors {
        let scope = if matches!(mode, Mode::Raid) {
            None
        } else {
            Some(anchor.source.clone())
        };
        scopes.entry(scope).or_default().push(anchor);
    }
    let mut result = Rendered::default();
    let mut bodies = Vec::new();
    let mut phase_ids = BTreeSet::new();
    for (name, anchors) in scopes {
        let scope_choices: Vec<_> = choices
            .iter()
            .filter(|choice| name.as_ref().is_none_or(|name| &choice.source == name))
            .collect();
        let (zeros, origins) = clocks(&anchors, progression, mode)?;
        result.nodes.extend(starts(mode, &origins));
        let (mut events, mut evidence) = schedule(&anchors, &zeros, mode, None, progression)?;
        let branches = branches::render(analysis, progression, &scope_choices)?;
        let has_branches = !branches.nodes.is_empty();
        result.loops.extend(loops::apply(
            analysis,
            progression,
            models,
            &origins,
            &mut events,
            &mut evidence,
            &name,
        ));
        // Place choices by their opening time so the generated schedule reads chronologically.
        for (choice, node) in scope_choices.iter().zip(branches.nodes) {
            let position = events
                .iter()
                .position(|event| {
                    event
                        .get(1)
                        .and_then(KdlValue::as_float)
                        .is_some_and(|at| at > choice.open as f64 / 10.0)
                })
                .unwrap_or(events.len());
            events.insert(position, node);
        }
        evidence.extend(branches.events);
        let has_loop = events.iter().any(|node| node.name().value() == "loop");
        let phase_id = name
            .as_deref()
            .map(slug)
            .or_else(|| (has_branches || has_loop).then(|| "combat".into()));
        result.origins.push(json!({"phase":phase_id,"policy":if matches!(mode,Mode::Raid){"combat-start"}else{"boss-cast"},
            "activities":origins.iter().map(|anchor|&anchor.activity).collect::<Vec<_>>(),"samples":zeros}));
        if let Some(phase_id) = phase_id {
            ensure!(
                phase_ids.insert(phase_id.clone()),
                "Boss phase ID collision: {phase_id}"
            );
            let mut phase = KdlNode::new("step");
            phase.push(phase_id.clone());
            if matches!(mode, Mode::Raid) {
                phase.ensure_children().nodes_mut().push(signal(
                    "enter",
                    "InCombat",
                    &[("inGameCombat", "1")],
                ));
            }
            for origin in &origins {
                phase.ensure_children().nodes_mut().push(signal(
                    "enter",
                    "Ability",
                    &[("id", &origin.ability), ("source", &origin.source)],
                ));
            }
            phase.ensure_children().nodes_mut().extend(events);
            for row in &mut evidence {
                row.as_object_mut()
                    .context("Invalid event evidence")?
                    .insert("phase".into(), phase_id.clone().into());
            }
            bodies.push((median(zeros.values().copied())?, phase_id, phase));
        } else {
            for event in events {
                bodies.push((0, String::new(), event));
            }
        }
        result.events.extend(evidence);
    }
    bodies.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    result.nodes.extend([
        signal("wipe", "ActorControl", &[("command", "4000000F")]),
        signal("wipe", "ActorControl", &[("command", "40000010")]),
        signal("zone-exit", "ChangeZone", &[]),
    ]);
    result
        .nodes
        .extend(bodies.into_iter().map(|(_, _, node)| node));
    Ok(result)
}

// Folded paths have no finite tail to fall back on, e.g. failed loop timing must not publish an entry-only fight.
fn check_repeats(draft: &Draft, rendered: &Rendered) -> Result<()> {
    for route in draft.routes.iter().filter(|route| route.repeat.is_some()) {
        ensure!(
            rendered
                .loops
                .iter()
                .any(|item| item["source"] == json!(route.source) && item["status"] == "rendered"),
            "Representative terminal repeat could not be rendered; no output was saved"
        );
    }
    Ok(())
}

// Mode is the only domain input; reading, mining, runtime policy, validation and publication stay internal.
pub(crate) fn generate(inputs: &[Utf8PathBuf], mode: Mode, output: &Path) -> Result<usize> {
    let analysis = super::mining::analyze(inputs)?;
    // Build occurrence structure before choosing printable rows or estimating any schedule.
    let progression = analysis.progression();
    let LoopDiscovery {
        models,
        diagnostics: loop_diagnostics,
    } = analysis.loop_models(!matches!(mode, Mode::Raid))?;
    let draft = analysis.draft(&progression, !matches!(mode, Mode::Raid), &models)?;
    let graph_choices = draft.has_choices() && draft.supports_observed_routes();
    let representative = analysis.needs_representative_route()
        || ((draft.has_choices() || draft.has_repeats()) && draft.supports_observed_routes());
    let Prepared {
        events: anchors,
        branches,
        mut excluded,
        diagnostics: mut alignment_diagnostics,
    } = if representative {
        ensure!(
            draft.supports_observed_routes(),
            "Representative routes differ; branch, internal-gap and ambiguous-prefix learning is not implemented for fewer than two clears"
        );
        draft_anchors(&analysis, &progression, &draft)?
    } else {
        anchors(&analysis, &progression, mode)?
    };
    alignment_diagnostics.push(analysis.draft_validation(!matches!(mode, Mode::Raid))?);
    let represented: BTreeSet<_> = anchors.iter().map(|anchor| anchor.source.clone()).collect();
    if !matches!(mode, Mode::Raid) {
        for boss in analysis.boss_names.difference(&represented) {
            excluded.push(json!({"boss":boss,"reason":"no unique common casts for this boss"}));
        }
    }
    let histories = if matches!(mode, Mode::Raid) && models.is_empty() && !graph_choices {
        analysis.history_models()?
    } else {
        Vec::new()
    };
    let preserved = if histories.is_empty() {
        Vec::new()
    } else {
        anchors.clone()
    };
    let mut rendered = compose(&analysis, &progression, anchors, branches, mode, &models)?;
    if representative {
        check_repeats(&draft, &rendered)?;
    }
    history::apply(
        &analysis,
        &progression,
        &histories,
        &preserved,
        &mut rendered,
    );
    excluded.retain(|row| {
        !rendered
            .events
            .iter()
            .any(|event| event["activity"] == row["activity"])
            && !rendered.loops.iter().any(|item| {
                item["activities"]
                    .as_array()
                    .is_some_and(|activities| activities.contains(&row["activity"]))
            })
    });
    let count = rendered.event_count();
    ensure!(count > 0, "No common cast events after the selected origin");
    let mut timeline_root = KdlNode::new("timeline");
    group_steps(&mut rendered)?;
    timeline_root.push(2_i128);
    timeline_root.push(analysis.name()?);
    timeline_root
        .ensure_children()
        .nodes_mut()
        .extend(rendered.nodes);
    let mut document = KdlDocument::new();
    document.nodes_mut().push(timeline_root);
    document.autoformat();
    let timeline = document.to_string();
    crate::timeline::parse(&timeline).context("Generated timeline failed validation")?;
    let timeline_sha256: String = ring::digest::digest(&ring::digest::SHA256, timeline.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let report = json!({"version":3,"mode":mode,"analysis":analysis.evidence(),
        "generation_method":if representative {"representative-route-v1"} else {"common-backbone-v1"},
        "progression":progression.evidence(),
        "draft":draft.evidence(),
        "timeline_sha256":timeline_sha256,"policy_reference":"cactbot@e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b",
        "origins":rendered.origins,
        "events":rendered.events,"excluded":excluded,"alignment_diagnostics":alignment_diagnostics,
        "loops":rendered.loops,"loop_diagnostics":loop_diagnostics,"histories":rendered.histories,
        "not_checked":["live ACT/runtime behavior","English name accuracy","missing-signal coverage","general branch and loop path conformance in automatic validation","general correlated choices and variable joins","internal boss phases and helper assignment","unclassified NPC casts as competing runtime signals","completion signals and content-specific lifecycle exceptions"]});
    crate::output::write_new(&[
        (output, timeline.as_bytes()),
        (
            &output.with_extension("evidence.json"),
            &serde_json::to_vec_pretty(&report)?,
        ),
    ])?;
    Ok(count)
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use serde_json::{Value, json};

use super::mining::{Analysis, Branch, CommonEvent, Identifier, Progression, Signal};
use super::timing::{self, median};

mod branches;

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
    for activity in analysis.signals().difference(&seen) {
        let reason = if activity.kind != "cast" {
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
    for anchor in anchors {
        let timing::Estimate {
            samples,
            median_ms: time,
            tick,
            min_tick,
            max_tick,
        } = timing::estimate(&progression.samples(&anchor.observations)?, zeros)?;
        let seconds = tick as f64 / 10.0;
        let mut event_id = format!(
            "{}-{}-{}",
            slug(&anchor.name),
            slug(&anchor.source),
            anchor.ability.to_lowercase()
        );
        if let Some(occurrence) = anchor.occurrence {
            event_id.push_str(&format!("-{occurrence:02}"));
        }
        if let Some(context) = context {
            event_id = format!("{context}-{event_id}");
        }
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
}

// Boss metadata supplies scope boundaries; travel and preceding trash never share a boss clock.
fn compose(
    analysis: &Analysis,
    progression: &Progression,
    anchors: Vec<Anchor>,
    choices: Vec<Branch>,
    mode: Mode,
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
        let (zeros, origins) = clocks(&anchors, progression, mode)?;
        result.nodes.extend(starts(mode, &origins));
        let (mut events, mut evidence) = schedule(&anchors, &zeros, mode, None, progression)?;
        let branches = branches::render(analysis, progression, &choices, &zeros)?;
        let has_branches = !branches.nodes.is_empty();
        // Place choices by their opening time so the generated schedule reads chronologically.
        for (choice, node) in choices.iter().zip(branches.nodes) {
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
        let phase_id = name
            .as_deref()
            .map(slug)
            .or_else(|| has_branches.then(|| "combat".into()));
        result.origins.push(json!({"phase":phase_id,"policy":if matches!(mode,Mode::Raid){"combat-start"}else{"boss-cast"},
            "activities":origins.iter().map(|anchor|&anchor.activity).collect::<Vec<_>>(),"samples":zeros}));
        if let Some(phase_id) = phase_id {
            ensure!(
                phase_ids.insert(phase_id.clone()),
                "Boss phase ID collision: {phase_id}"
            );
            let mut phase = KdlNode::new("phase");
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

// Mode is the only domain input; reading, mining, runtime policy, validation and publication stay internal.
pub(crate) fn generate(inputs: &[Utf8PathBuf], mode: Mode, output: &Path) -> Result<usize> {
    let analysis = super::mining::analyze(inputs)?;
    // Build occurrence structure before choosing printable rows or estimating any schedule.
    let progression = analysis.progression();
    let Prepared {
        events: anchors,
        branches,
        mut excluded,
        diagnostics: alignment_diagnostics,
    } = anchors(&analysis, &progression, mode)?;
    let represented: BTreeSet<_> = anchors.iter().map(|anchor| anchor.source.clone()).collect();
    if !matches!(mode, Mode::Raid) {
        for boss in analysis.boss_names.difference(&represented) {
            excluded.push(json!({"boss":boss,"reason":"no unique common casts for this boss"}));
        }
    }
    let rendered = compose(&analysis, &progression, anchors, branches, mode)?;
    ensure!(
        !rendered.events.is_empty(),
        "No common cast events after the selected origin"
    );
    let count = rendered.events.len();
    let mut timeline_root = KdlNode::new("timeline");
    timeline_root.push(1_i128);
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
    let report = json!({"version":2,"mode":mode,"analysis":analysis.evidence(),
        "progression":progression.evidence(),
        "timeline_sha256":timeline_sha256,"policy_reference":"cactbot@e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b",
        "origins":rendered.origins,
        "events":rendered.events,"excluded":excluded,"alignment_diagnostics":alignment_diagnostics,
        "not_checked":["live ACT/runtime behavior","English name accuracy","missing-signal coverage","branch path conformance in automatic validation","correlated choices, variable joins and loops","internal boss phases and helper assignment","completion signals and content-specific lifecycle exceptions"]});
    crate::output::write_new(&[
        (output, timeline.as_bytes()),
        (
            &output.with_extension("evidence.json"),
            &serde_json::to_vec_pretty(&report)?,
        ),
    ])?;
    Ok(count)
}

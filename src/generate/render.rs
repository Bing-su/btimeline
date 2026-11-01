use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use kdl::{KdlDocument, KdlEntry, KdlNode, KdlValue};
use serde_json::{Value, json};

use super::mining::{Analysis, Identifier, Signal};

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

fn median(values: impl Iterator<Item = i64>) -> Result<i64> {
    let mut values: Vec<_> = values.collect();
    ensure!(!values.is_empty(), "Missing timing samples");
    values.sort_unstable();
    values
        .get((values.len() - 1) / 2)
        .copied()
        .context("Missing median")
}

struct Anchor {
    activity: Signal,
    name: String,
    source: String,
    ability: String,
    samples: BTreeMap<String, i64>,
    occurrence: Option<usize>,
    alignment: Value,
}

// Consume the shared in-memory mining result; generation never requires a serialized graph input.
fn anchors(analysis: &Analysis, mode: Mode) -> Result<(Vec<Anchor>, Vec<Value>, Vec<Value>)> {
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
    let common = analysis.aligned(include)?;
    let mut anchors = Vec::new();
    let mut seen = BTreeSet::new();
    let mut excluded = Vec::new();
    for row in common.events {
        let activity = row.signal;
        seen.insert(activity.clone());
        let Identifier::Ability(ability) = activity.identifier else {
            anyhow::bail!("Cast requires an ability ID");
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
        anchors.push(Anchor {
            activity,
            name,
            source,
            ability: format!("{ability:X}"),
            samples: row.samples,
            occurrence: row.occurrence,
            alignment: row.evidence,
        });
    }
    for activity in analysis.signals().difference(&seen) {
        let reason = if activity.kind != "cast" {
            "non-cast activity"
        } else if !include(activity) {
            "not a Boss actor"
        } else {
            "no forced common occurrence"
        };
        excluded.push(json!({"activity":activity,"reason":reason}));
    }
    ensure!(!anchors.is_empty(), "No unique common casts found");
    Ok((anchors, excluded, common.diagnostics))
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

fn clocks(anchors: &[Anchor], mode: Mode) -> Result<(BTreeMap<String, i64>, Vec<&Anchor>)> {
    let first = anchors.first().context("Missing timing samples")?;
    let mut zeros = BTreeMap::new();
    for trace in first.samples.keys() {
        let zero = match mode {
            Mode::Raid => 0,
            _ => *anchors
                .iter()
                .filter_map(|anchor| anchor.samples.get(trace))
                .min()
                .context("Missing boss origin")?,
        };
        zeros.insert(trace.clone(), zero);
    }
    let origins = match mode {
        Mode::Raid => Vec::new(),
        _ => anchors
            .iter()
            .filter(|anchor| {
                anchor
                    .samples
                    .iter()
                    .any(|(trace, at)| zeros.get(trace) == Some(at))
            })
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

#[derive(serde::Serialize)]
struct Sample<'a> {
    trace: &'a str,
    at_ms: i64,
    origin_ms: i64,
    offset_ms: i64,
}

fn schedule(
    anchors: &[Anchor],
    zeros: &BTreeMap<String, i64>,
    mode: Mode,
) -> Result<(Vec<KdlNode>, Vec<Value>)> {
    let mut events = Vec::new();
    let mut evidence = Vec::new();
    let mut used = BTreeSet::new();
    for anchor in anchors {
        let samples = anchor
            .samples
            .iter()
            .map(|(trace, at)| {
                let zero = zeros.get(trace).context("Missing origin sample")?;
                Ok(Sample {
                    trace,
                    at_ms: *at,
                    origin_ms: *zero,
                    offset_ms: at - zero,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            samples
                .iter()
                .all(|sample| (0..=crate::timeline::MAX_MS).contains(&sample.offset_ms)),
            "Sample exceeds supported time range"
        );
        let time = median(samples.iter().map(|sample| sample.offset_ms))?;
        ensure!(
            time <= crate::timeline::MAX_MS,
            "Schedule exceeds supported time range"
        );
        let tick = (time + 50) / 100;
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
        let min_tick = samples
            .iter()
            .map(|sample| (sample.offset_ms + 50) / 100)
            .min()
            .context("Missing sync samples")?;
        let max_tick = samples
            .iter()
            .map(|sample| (sample.offset_ms + 50) / 100)
            .max()
            .context("Missing sync samples")?;
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
            "alignment":anchor.alignment}),
        );
    }
    events.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    // Cactbot opening anchors use wider windows to correct the initial combat-clock estimate.
    if matches!(mode, Mode::Raid)
        && let Some((_, _, event)) = events.first_mut()
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
fn compose(anchors: Vec<Anchor>, mode: Mode) -> Result<Rendered> {
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
        let (zeros, origins) = clocks(&anchors, mode)?;
        result.nodes.extend(starts(mode, &origins));
        let (events, mut evidence) = schedule(&anchors, &zeros, mode)?;
        let phase_id = name.as_deref().map(slug);
        result.origins.push(json!({"phase":phase_id,"policy":if matches!(mode,Mode::Raid){"combat-start"}else{"boss-cast"},
            "activities":origins.iter().map(|anchor|&anchor.activity).collect::<Vec<_>>(),"samples":zeros}));
        if let Some(phase_id) = phase_id {
            ensure!(
                phase_ids.insert(phase_id.clone()),
                "Boss phase ID collision: {phase_id}"
            );
            let mut phase = KdlNode::new("phase");
            phase.push(phase_id.clone());
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
    let (anchors, mut excluded, alignment_diagnostics) = anchors(&analysis, mode)?;
    let represented: BTreeSet<_> = anchors.iter().map(|anchor| anchor.source.clone()).collect();
    if !matches!(mode, Mode::Raid) {
        for boss in analysis.boss_names.difference(&represented) {
            excluded.push(json!({"boss":boss,"reason":"no unique common casts for this boss"}));
        }
    }
    let rendered = compose(anchors, mode)?;
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
        "timeline_sha256":timeline_sha256,"policy_reference":"cactbot@e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b",
        "origins":rendered.origins,
        "events":rendered.events,"excluded":excluded,"alignment_diagnostics":alignment_diagnostics,
        "not_checked":["live ACT/runtime behavior","English name accuracy","missing-signal coverage","branches and loops","internal boss phases and helper assignment","completion signals and content-specific lifecycle exceptions"]});
    crate::output::write_new(&[
        (output, timeline.as_bytes()),
        (
            &output.with_extension("evidence.json"),
            &serde_json::to_vec_pretty(&report)?,
        ),
    ])?;
    Ok(count)
}

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use kdl::KdlNode;
use serde_json::{Value, json};

use super::{Analysis, Anchor, Identifier, LoopModel, Progression, Signal, prop, slug};
use crate::generate::timing::median;

struct Spec {
    position: usize,
    signals: Vec<Signal>,
    variants: Vec<Vec<Signal>>,
}
struct Scheduled {
    spec: Spec,
    tick: i64,
    before: i64,
    after: i64,
}
pub(super) struct LoopRender {
    pub node: KdlNode,
    pub evidence: Value,
    pub start_tick: i64,
}

fn replace(
    rendered: LoopRender,
    events: &mut Vec<KdlNode>,
    evidence: &mut Vec<Value>,
    context: Option<&Signal>,
) -> Result<Value> {
    // Local onset learning is usable only if the standalone KDL can actually observe that context.
    ensure!(
        context.is_none_or(|context| evidence.iter().any(|row| {
            row["activity"] == json!(context)
                && events.iter().any(|event| {
                    event.get(0).and_then(kdl::KdlValue::as_string) == row["event"].as_str()
                        && event.children().is_some_and(|body| {
                            body.nodes()
                                .iter()
                                .any(|node| node.name().value() == "sync")
                        })
                })
        })),
        "Loop onset context has no finite synchronization"
    );
    let covered: BTreeSet<_> = rendered
        .evidence
        .get("events")
        .context("Missing loop events")?
        .as_array()
        .context("Missing loop events")?
        .iter()
        .flat_map(|event| event["observations"].as_array().into_iter().flatten())
        .flat_map(|row| {
            row["observation_nodes"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|node| (row["trace"].clone(), node.clone()))
        })
        .map(|(trace, node)| {
            Ok((
                trace.as_str().context("Missing trace")?.to_owned(),
                node.as_u64().context("Missing node")?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut ids = BTreeSet::new();
    for row in evidence.iter() {
        let nodes = row["observation_nodes"]
            .as_object()
            .context("Missing finite observations")?;
        let replaced = nodes
            .iter()
            .filter(|(trace, node)| {
                node.as_u64()
                    .is_some_and(|node| covered.contains(&((*trace).clone(), node)))
            })
            .count();
        ensure!(
            replaced == 0 || replaced == nodes.len(),
            "Finite occurrence crosses loop onset"
        );
        if replaced > 0 {
            ids.insert(
                row["event"]
                    .as_str()
                    .context("Missing event ID")?
                    .to_owned(),
            );
        } else {
            ensure!(
                row["rounded_offset_ms"]
                    .as_i64()
                    .is_some_and(|at| at < rendered.start_tick * 100),
                "Finite opener overlaps loop onset"
            );
        }
    }
    let mut replaced = Vec::new();
    evidence.retain(|row| {
        let keep = row["event"].as_str().is_none_or(|id| !ids.contains(id));
        if !keep {
            replaced.push(row.clone());
        }
        keep
    });
    events.retain(|node| {
        node.get(0)
            .and_then(kdl::KdlValue::as_string)
            .is_none_or(|id| !ids.contains(id))
    });
    events.push(rendered.node);
    let mut report = rendered.evidence;
    report
        .as_object_mut()
        .context("Invalid loop evidence")?
        .insert("replaced_finite_events".into(), json!(replaced));
    Ok(report)
}

pub(super) fn apply(
    analysis: &Analysis,
    progression: &Progression,
    models: &[LoopModel],
    origins: &[&Anchor],
    events: &mut Vec<KdlNode>,
    evidence: &mut Vec<Value>,
    source: &Option<String>,
) -> Vec<Value> {
    let mut reports = Vec::new();
    for model in models.iter().filter(|model| source == &model.source) {
        match render(analysis, progression, model, origins)
            .and_then(|rendered| replace(rendered, events, evidence, model.context.as_ref())) {
            Ok(report) => reports.push(report),
            Err(error) => reports.push(
                json!({"source":model.source,"status":"unrendered","model":model,"reason":error.to_string()}),
            ),
        }
    }
    reports
}

// Long traces do not gain more timing weight merely because they observe more iterations.
fn stats(samples: impl Iterator<Item = (String, i64)>) -> Result<(i64, i64, i64)> {
    let mut traces = BTreeMap::<_, Vec<_>>::new();
    for (trace, value) in samples {
        ensure!(
            (0..=crate::timeline::MAX_MS).contains(&value),
            "Unsupported loop timing sample"
        );
        traces.entry(trace).or_default().push(value);
    }
    let values = traces
        .values()
        .map(|values| median(values.iter().copied()))
        .collect::<Result<Vec<_>>>()?;
    let center = (median(values.iter().copied())? + 50) / 100;
    let all: Vec<_> = traces
        .values()
        .flatten()
        .map(|value| (value + 50) / 100)
        .collect();
    Ok((
        center,
        *all.iter().min().context("Missing loop timings")?,
        *all.iter().max().context("Missing loop timings")?,
    ))
}

fn specs(model: &LoopModel) -> Result<Vec<Spec>> {
    let mut result = Vec::new();
    for block in &model.blocks {
        let variants = block
            .iter()
            .map(|index| model.body.get(*index).context("Invalid loop block"))
            .collect::<Result<Vec<_>>>()?;
        let first = variants.first().context("Empty loop block")?;
        let common: BTreeSet<_> = first
            .iter()
            .filter(|signal| variants.iter().all(|variant| variant.contains(signal)))
            .cloned()
            .collect();
        let remainder: Vec<_> = variants
            .iter()
            .map(|variant| {
                variant
                    .iter()
                    .filter(|signal| !common.contains(*signal))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect();
        ensure!(
            remainder.iter().all(Vec::is_empty)
                || remainder.iter().all(|variant| !variant.is_empty()),
            "Optional coincident loop signals need conditional modeling"
        );
        for position in block {
            for signal in &common {
                result.push(Spec {
                    position: *position,
                    signals: vec![signal.clone()],
                    variants: vec![vec![signal.clone()]],
                });
            }
            if remainder.iter().any(|variant| !variant.is_empty()) {
                let signals = remainder
                    .iter()
                    .flatten()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                result.push(Spec {
                    position: *position,
                    signals,
                    variants: remainder.clone(),
                });
            }
        }
    }
    Ok(result)
}

fn schedule(model: &LoopModel) -> Result<(i64, i64, Vec<Scheduled>)> {
    let width = model.body.len();
    let start = stats(
        model
            .episodes
            .iter()
            .map(|ep| {
                Ok((
                    ep.trace.clone(),
                    ep.waves.first().context("Empty loop episode")?.at()? - ep.phase_origin_ms,
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter(),
    )?
    .0;
    let mut periods = Vec::new();
    for ep in &model.episodes {
        for index in (width..ep.waves.len()).step_by(width) {
            periods.push((
                ep.trace.clone(),
                ep.waves
                    .get(index)
                    .context("Missing cycle boundary")?
                    .at()?
                    - ep.waves
                        .get(index - width)
                        .context("Missing cycle origin")?
                        .at()?,
            ));
        }
    }
    let period = stats(periods.into_iter())?.0;
    ensure!(period > 0, "Nonpositive loop period");
    let mut result = Vec::new();
    for spec in specs(model)? {
        let mut samples = Vec::new();
        for ep in &model.episodes {
            for (index, wave) in ep
                .waves
                .iter()
                .enumerate()
                .filter(|(index, _)| index % width == spec.position)
            {
                let origin = ep
                    .waves
                    .get(index / width * width)
                    .context("Missing cycle origin")?
                    .at()?;
                samples.push((ep.trace.clone(), wave.at()? - origin));
            }
        }
        let (tick, min, max) = stats(samples.into_iter())?;
        ensure!(tick < period, "Loop event exceeds the inferred cycle");
        result.push(Scheduled {
            spec,
            tick,
            before: (tick - min).max(25),
            after: (max - tick).max(25),
        });
    }
    Ok((start, period, result))
}

fn label(analysis: &Analysis, spec: &Spec) -> Result<String> {
    let variants = spec
        .variants
        .iter()
        .map(|variant| {
            let names = variant
                .iter()
                .map(|signal| {
                    let Identifier::Ability(id) = signal.identifier else {
                        anyhow::bail!("Loop needs ability identifiers")
                    };
                    let names = analysis
                        .ability_names
                        .get(&id)
                        .context("Missing loop ability name")?;
                    ensure!(names.len() == 1, "Ambiguous loop ability name");
                    names.first().cloned().context("Missing loop name")
                })
                .collect::<Result<BTreeSet<_>>>()?;
            Ok(names.into_iter().collect::<Vec<_>>().join(" + "))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(variants.into_iter().collect::<Vec<_>>().join(" / "))
}

fn observations(model: &LoopModel, spec: &Spec, progression: &Progression) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    for ep in &model.episodes {
        for (index, wave) in ep
            .waves
            .iter()
            .enumerate()
            .filter(|(index, _)| index % model.body.len() == spec.position)
        {
            let members: Vec<_> = wave
                .members
                .iter()
                .filter(|member| spec.signals.contains(&member.signal))
                .collect();
            let mut nodes = BTreeSet::new();
            for member in &members {
                nodes.extend(
                    progression
                        .bind(
                            &member.signal,
                            &BTreeMap::from([(ep.trace.clone(), member.at_ms)]),
                        )?
                        .into_values(),
                );
            }
            result.push(json!({"trace":ep.trace,"actor":ep.actor,"instance":ep.instance,"iteration":index/model.body.len()+1,
                "cycle_start_ms":ep.waves.get(index/model.body.len()*model.body.len()).context("Missing cycle origin")?.at()?,"members":members,"observation_nodes":nodes}));
        }
    }
    Ok(result)
}

// Competing identities are checked across the cycle boundary too; unsafe syncs remain display-only.
fn safe(index: usize, rows: &[Scheduled], period: i64) -> bool {
    let Some(row) = rows.get(index) else {
        return false;
    };
    rows.iter()
        .enumerate()
        .filter(|(other, _)| *other != index)
        .all(|(_, other)| {
            !row.spec
                .signals
                .iter()
                .any(|signal| other.spec.signals.contains(signal))
                || [-period, 0, period].into_iter().all(|shift| {
                    row.tick + row.after < other.tick + shift - other.before
                        || other.tick + shift + other.after < row.tick - row.before
                })
        })
}

pub(super) fn render(
    analysis: &Analysis,
    progression: &Progression,
    model: &LoopModel,
    origins: &[&Anchor],
) -> Result<LoopRender> {
    let entry = model
        .prefix
        .first()
        .or_else(|| model.body.first())
        .context("Missing loop entry")?;
    ensure!(
        model.source.is_none()
            || (!origins.is_empty()
                && (model.context.is_some()
                    || (entry.len() == 1
                        && origins
                            .iter()
                            .all(|origin| entry.contains(&origin.activity))))),
        "Loop entry is not consistently represented by the phase origin"
    );
    for origin in origins {
        ensure!(
            progression
                .samples(&origin.observations)?
                .iter()
                .all(|(trace, at)| model
                    .episodes
                    .iter()
                    .any(|ep| ep.trace == *trace && ep.phase_origin_ms == *at)),
            "Finite phase origin differs from the observed loop context"
        );
    }
    let (start, period, rows) = schedule(model)?;
    let id = format!(
        "{}-rotation",
        model
            .source
            .as_deref()
            .map_or_else(|| "combat".into(), slug)
    );
    let mut node = KdlNode::new("loop");
    node.push(id.clone());
    prop(&mut node, "start", start as f64 / 10.0);
    prop(&mut node, "every", period as f64 / 10.0);
    let mut events = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = label(analysis, &row.spec)?;
        let ids: BTreeSet<_> = row
            .spec
            .signals
            .iter()
            .filter_map(|signal| match signal.identifier {
                Identifier::Ability(id) => Some(format!("{id:X}")),
                _ => None,
            })
            .collect();
        let event_id = format!(
            "{id}-{:02}-{}{}-{}",
            row.spec.position + 1,
            slug(&name),
            if model.source.is_none() {
                format!(
                    "-{}",
                    row.spec
                        .signals
                        .iter()
                        .filter_map(|signal| signal.source.as_deref())
                        .map(slug)
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join("-")
                )
            } else {
                String::new()
            },
            ids.iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("-")
                .to_lowercase()
        );
        let mut event = KdlNode::new("event");
        event.push(event_id.clone());
        event.push(row.tick as f64 / 10.0);
        prop(&mut event, "name", name);
        let sync = safe(index, &rows, period);
        if sync {
            event.ensure_children().nodes_mut().extend(syncs(row)?);
        }
        node.ensure_children().nodes_mut().push(event);
        events.push(json!({"event":event_id,"offset_ms":row.tick*100,"activities":row.spec.signals,"sync":sync,
            "observations":observations(model,&row.spec,progression)?}));
    }
    let activities: BTreeSet<_> = model.body.iter().flatten().collect();
    Ok(LoopRender {
        node,
        start_tick: start,
        evidence: json!({"id":id,"source":model.source,"status":"rendered","start_ms":start*100,"period_ms":period*100,
        "activities":activities,"events":events,"model":model,"not_checked":["ACT runtime","conditional path semantics","exit signal generation"]}),
    })
}

// Keep source/ID pairs together; an OR across both fields would invent unobserved combinations.
fn syncs(row: &Scheduled) -> Result<Vec<KdlNode>> {
    let mut by_source = BTreeMap::<_, BTreeSet<_>>::new();
    for activity in &row.spec.signals {
        let source = activity
            .source
            .as_deref()
            .context("Missing loop source name")?;
        let Identifier::Ability(id) = activity.identifier else {
            anyhow::bail!("Loop needs ability identifiers")
        };
        by_source
            .entry(source)
            .or_default()
            .insert(format!("{id:X}"));
    }
    Ok(by_source
        .into_iter()
        .map(|(source, ids)| {
            let mut signal = KdlNode::new("sync");
            signal.push(row.tick as f64 / 10.0);
            signal.push("Ability");
            prop(&mut signal, "source", source);
            prop(&mut signal, "before", row.before as f64 / 10.0);
            prop(&mut signal, "after", row.after as f64 / 10.0);
            let mut field = KdlNode::new("field");
            field.push("id");
            for id in ids {
                field.push(id);
            }
            signal.ensure_children().nodes_mut().push(field);
            signal
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_alternatives_preserve_source_and_id_pairs() {
        let signals: Vec<_> = [("Left Helper", 1), ("Right Helper", 2)]
            .into_iter()
            .map(|(source, id)| Signal {
                kind: "cast".into(),
                identifier: Identifier::Ability(id),
                source: Some(source.into()),
                target: None,
            })
            .collect();
        let row = Scheduled {
            spec: Spec {
                position: 0,
                variants: signals.iter().cloned().map(|signal| vec![signal]).collect(),
                signals,
            },
            tick: 10,
            before: 25,
            after: 25,
        };
        let alternatives = syncs(&row).unwrap();
        assert_eq!(alternatives.len(), 2);
        for (sync, (source, id)) in alternatives
            .iter()
            .zip([("Left Helper", "1"), ("Right Helper", "2")])
        {
            assert_eq!(
                sync.get("source").and_then(kdl::KdlValue::as_string),
                Some(source)
            );
            assert_eq!(
                sync.children().unwrap().nodes()[0]
                    .get(1)
                    .and_then(kdl::KdlValue::as_string),
                Some(id)
            );
            assert!(sync.children().unwrap().nodes()[0].get(2).is_none());
        }
    }

    #[test]
    fn local_onset_without_an_emitted_sync_preserves_finite_output() {
        let context = Signal {
            kind: "cast".into(),
            identifier: Identifier::Ability(9),
            source: Some("Boss".into()),
            target: None,
        };
        let mut evidence = vec![json!({"event":"entry","activity":context})];
        let original = evidence.clone();
        let mut events = vec![KdlNode::new("event")];
        let rendered = LoopRender {
            node: KdlNode::new("loop"),
            start_tick: 10,
            evidence: json!({}),
        };
        let error = replace(rendered, &mut events, &mut evidence, Some(&context)).unwrap_err();
        assert!(error.to_string().contains("no finite synchronization"));
        assert_eq!(events.len(), 1);
        assert_eq!(evidence, original);
    }

    #[test]
    fn partial_loop_binding_does_not_remove_finite_events() {
        let mut events = vec![KdlNode::new("event")];
        let mut evidence = vec![
            json!({"event":"finite","rounded_offset_ms":2000,"observation_nodes":{"a":1,"b":2}}),
        ];
        let original = evidence.clone();
        let rendered = LoopRender {
            node: KdlNode::new("loop"),
            start_tick: 10,
            evidence: json!({"events":[{"observations":[{"trace":"a","observation_nodes":[1]}]}]}),
        };
        let error = replace(rendered, &mut events, &mut evidence, None).unwrap_err();
        assert!(error.to_string().contains("crosses loop onset"));
        assert_eq!(events.len(), 1);
        assert_eq!(evidence, original);
    }

    #[test]
    fn long_traces_do_not_dominate_period_estimation() {
        let mut samples = vec![("long".to_owned(), 1000); 100];
        samples.extend([("short-a".into(), 2000), ("short-b".into(), 3000)]);
        assert_eq!(stats(samples.into_iter()).unwrap(), (20, 10, 30));
    }
}

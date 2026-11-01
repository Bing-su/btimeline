use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Pull, Signal, alignment};

mod attempts;
mod branches;

pub(in crate::generate) struct Event {
    pub signal: Signal,
    pub samples: BTreeMap<String, i64>,
    pub occurrence: Option<usize>,
    pub evidence: Value,
}

pub(in crate::generate) struct Common {
    pub events: Vec<Event>,
    pub diagnostics: Vec<Value>,
    pub branches: Vec<Branch>,
}

pub(in crate::generate) struct Branch {
    pub source: String,
    pub join: Signal,
    pub open: i64,
    pub close: i64,
    pub options: Vec<BranchOption>,
}

pub(in crate::generate) struct BranchOption {
    pub trigger: Signal,
    pub triggers: BTreeMap<String, i64>,
    pub events: Vec<Event>,
}

struct Observation<'a> {
    signal: &'a Signal,
    batch: usize,
    at: i64,
    ordinal: usize,
}

struct Group<'a> {
    observations: Vec<Observation<'a>>,
    key: Vec<&'a Signal>,
}

// Forget internal order conservatively, e.g. A@0,B@80,C@160 form one uncertainty block.
fn groups(observations: Vec<Observation<'_>>) -> Vec<Group<'_>> {
    let mut groups: Vec<Group<'_>> = Vec::new();
    for observation in observations {
        let previous = groups.last_mut().filter(|group| {
            group
                .observations
                .last()
                .is_some_and(|last| observation.at - last.at <= super::common::ORDER_RESOLUTION_MS)
        });
        if let Some(group) = previous {
            group.key.push(observation.signal);
            group.observations.push(observation);
        } else {
            groups.push(Group {
                key: vec![observation.signal],
                observations: vec![observation],
            });
        }
    }
    for group in &mut groups {
        group.key.sort_unstable();
    }
    groups
}

impl Group<'_> {
    fn unique(&self, signal: &Signal) -> Option<&Observation<'_>> {
        let mut matches = self.observations.iter().filter(|row| row.signal == signal);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }
}

fn trace(pull: &Pull) -> String {
    format!("{}/{}", pull.report, pull.fight)
}

fn interval<'a>(
    pull: &'a Pull,
    left: i64,
    right: Option<i64>,
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
) -> Result<Vec<Group<'a>>, &'static str> {
    let end = right.unwrap_or(pull.end_ms);
    if pull
        .phase_transitions
        .iter()
        .any(|phase| left < phase.relative_ms && phase.relative_ms < end)
    {
        return Err("phase boundary");
    }
    let mut counts = BTreeMap::new();
    let mut result = Vec::new();
    for (batch, frame) in pull.batches.iter().enumerate() {
        let selected: Vec<_> = frame
            .waves()
            .into_iter()
            .filter(|signal| &signal.source == source && include(signal))
            .collect();
        for signal in &selected {
            *counts.entry(*signal).or_insert(0) += 1;
        }
        if frame.at_ms < left || frame.at_ms > end || selected.is_empty() {
            continue;
        }
        if selected.len() > 1 && (frame.at_ms == left || Some(frame.at_ms) == right) {
            return Err("unordered boundary");
        }
        if frame.at_ms == left || Some(frame.at_ms) == right {
            continue;
        }
        if frame.at_ms - left <= super::common::ORDER_RESOLUTION_MS
            || right.is_some_and(|right| right - frame.at_ms <= super::common::ORDER_RESOLUTION_MS)
        {
            return Err("unordered boundary");
        }
        for signal in selected {
            result.push(Observation {
                signal,
                batch,
                at: frame.at_ms,
                ordinal: *counts.get(signal).ok_or("missing ordinal")?,
            });
        }
    }
    Ok(groups(result))
}

fn unrepresented(pull: &Pull, events: &[&Event], include: &impl Fn(&Signal) -> bool) -> Value {
    let id = trace(pull);
    let rows: Vec<_> = pull
        .batches
        .iter()
        .enumerate()
        .flat_map(|(batch, frame)| {
            frame
                .signals
                .iter()
                .filter(|signal| include(signal))
                .filter(|signal| {
                    !events.iter().any(|event| {
                        &event.signal == *signal && event.samples.get(&id) == Some(&frame.at_ms)
                    })
                })
                .map(move |signal| json!({"batch":batch,"activity":signal}))
        })
        .collect();
    json!({"trace":id,"unrepresented":rows,"reason":"no forced bounded correspondence"})
}

fn pair(reference: &[Group<'_>], peer: &[Group<'_>]) -> Result<alignment::Alignment> {
    alignment::align(
        &reference.iter().map(|row| &row.key).collect::<Vec<_>>(),
        &peer.iter().map(|row| &row.key).collect::<Vec<_>>(),
    )
}

fn batches(rows: &[Group<'_>], indices: &[usize]) -> Vec<usize> {
    indices
        .iter()
        .filter_map(|i| rows.get(*i))
        .flat_map(|group| group.observations.iter().map(|row| row.batch))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn events(pulls: &[&Pull], groups: &[&Group<'_>], bounds: &Value) -> Result<Vec<Event>> {
    let reference = groups.first().context("Missing reference group")?;
    let blocks: BTreeMap<_, _> = pulls
        .iter()
        .zip(groups)
        .map(|(pull, group)| {
            (
                trace(pull),
                group
                    .observations
                    .iter()
                    .map(|row| row.batch)
                    .collect::<std::collections::BTreeSet<_>>(),
            )
        })
        .collect();
    let mut result = Vec::new();
    for observation in &reference.observations {
        // B,B in one uncertainty block has no forced occurrence correspondence, even in equal blocks.
        let observations = groups
            .iter()
            .map(|group| group.unique(observation.signal))
            .collect::<Option<Vec<_>>>();
        let Some(observations) = observations else {
            continue;
        };
        let samples = pulls
            .iter()
            .zip(&observations)
            .map(|(pull, row)| (trace(pull), row.at))
            .collect();
        let batches: BTreeMap<_, _> = pulls
            .iter()
            .zip(&observations)
            .map(|(pull, row)| (trace(pull), row.batch))
            .collect();
        result.push(Event {signal:observation.signal.clone(),samples,occurrence:Some(observation.ordinal),
            evidence:json!({"method":"anchor-constrained-multiset-lcs-v2","bounds":bounds,"reference":trace(pulls.first().context("Missing reference")?),"batches":batches,"groups":blocks,"sync":"omitted: repeated signal identity"})});
    }
    Ok(result)
}

fn expand_interval(
    pulls: &[&Pull],
    rows: &[&[Group<'_>]],
    bounds: Value,
    result: &mut Common,
) -> Result<()> {
    let reference = rows.first().context("Missing reference interval")?;
    if rows
        .iter()
        .any(|row| !alignment::within_limit(reference.len(), row.len()))
    {
        result
            .diagnostics
            .push(json!({"bounds":bounds,"status":"resource-limit"}));
        return Ok(());
    }
    let matches = rows
        .iter()
        .map(|row| pair(reference, row))
        .collect::<Result<Vec<_>>>()?;
    for ((pull, row), aligned) in pulls.iter().zip(rows).zip(&matches) {
        result.diagnostics.push(json!({"bounds":bounds,"reference":trace(pulls.first().context("Missing reference")?),"trace":trace(pull),"optimal_group_matches":aligned.optimal_matches,
            "ambiguous_reference":batches(reference,&aligned.ambiguous_left),
            "reference_only":batches(reference,&aligned.reference_only),
            "ambiguous_peer":batches(row,&aligned.ambiguous_right),"peer_only":batches(row,&aligned.peer_only)}));
    }
    for index in 0..reference.len() {
        let peers = matches
            .iter()
            .map(|aligned| {
                aligned
                    .pairs
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map(|(_, j)| *j)
            })
            .collect::<Option<Vec<_>>>();
        let Some(peers) = peers else { continue };
        let groups = rows
            .iter()
            .zip(peers)
            .map(|(row, j)| row.get(j).context("Missing aligned observation"))
            .collect::<Result<Vec<_>>>()?;
        result.events.extend(events(pulls, &groups, &bounds)?);
    }
    Ok(())
}

fn completed<'a>(pulls: &[&'a Pull], include: &impl Fn(&Signal) -> bool) -> Vec<&'a Pull> {
    let mut completed: Vec<_> = pulls.iter().copied().filter(|pull| pull.kill).collect();
    // The longest completed trace supplies readable occurrence labels; ties ignore CLI argument order.
    completed.sort_by_key(|pull| {
        (
            std::cmp::Reverse(
                pull.batches
                    .iter()
                    .flat_map(|b| b.waves())
                    .filter(|s| include(s))
                    .count(),
            ),
            trace(pull),
        )
    });
    completed
}

pub(super) fn holdout_checks(
    training: &[&Pull],
    held_out: &[&Pull],
    events: &[Event],
    include: &impl Fn(&Signal) -> bool,
) -> Result<Vec<Value>> {
    let completed = completed(training, include);
    let reference = completed.first().context("Missing training reference")?;
    let mut results = attempts::checks(held_out.iter().copied(), reference, events, include)?;
    for row in &mut results {
        row.as_object_mut()
            .context("Invalid holdout check")?
            .insert("held_out".into(), true.into());
    }
    Ok(results)
}

pub(super) fn expand(
    pulls: &[&Pull],
    unique: BTreeMap<Signal, BTreeMap<String, i64>>,
    include: impl Fn(&Signal) -> bool,
    raid: bool,
) -> Result<Common> {
    let completed = completed(pulls, &include);
    let reference = trace(completed.first().context("Missing reference")?);
    let table = unique
        .iter()
        .map(|(signal, samples)| (signal.clone(), samples.values().copied().collect()))
        .collect();
    let backbone = super::common::backbone(&table);
    let all_unique = unique;
    let unique: BTreeMap<_, _> = all_unique
        .iter()
        .filter(|(signal, _)| backbone.contains(*signal))
        .map(|(signal, samples)| (signal.clone(), samples.clone()))
        .collect();
    let mut result = Common {
        events: unique
            .iter()
            .map(|(signal, samples)| Event {
                signal: signal.clone(),
                samples: samples.clone(),
                occurrence: None,
                evidence: Value::Null,
            })
            .collect(),
        diagnostics: Vec::new(),
        branches: Vec::new(),
    };
    let mut scopes = BTreeMap::<_, Vec<_>>::new();
    for (signal, samples) in &unique {
        scopes
            .entry(&signal.source)
            .or_default()
            .push((signal, samples));
    }
    for (source, mut anchors) in scopes {
        anchors.sort_by_key(|(_, samples)| samples.get(&reference));
        for window in anchors.windows(2) {
            let [(left, a), (right, b)] = window else {
                continue;
            };
            let bounds = json!({"left":left,"right":right});
            let rows = completed
                .iter()
                .map(|pull| {
                    let id = trace(pull);
                    let start = a.get(&id).ok_or((id.clone(), "missing bound"))?;
                    let end = b.get(&id).ok_or((id.clone(), "missing bound"))?;
                    interval(pull, *start, Some(*end), source, &include)
                        .map_err(|reason| (id, reason))
                })
                .collect::<Result<Vec<_>, _>>();
            match rows {
                Ok(rows) => expand_interval(
                    &completed,
                    &rows.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                    bounds,
                    &mut result,
                )?,
                Err((trace, reason)) => result.diagnostics.push(
                    json!({"bounds":bounds,"trace":trace,"status":"unresolved","reason":reason}),
                ),
            }
        }
    }
    branches::discover(
        &completed,
        &all_unique,
        &unique,
        &include,
        raid,
        &mut result,
    )?;
    // A represented B-02 must not hide B-01 outside the bounds or in an ambiguous interval.
    let represented: Vec<_> = result
        .events
        .iter()
        .chain(
            result
                .branches
                .iter()
                .flat_map(|branch| &branch.options)
                .flat_map(|option| &option.events),
        )
        .collect();
    result.diagnostics.extend(
        completed
            .iter()
            .map(|pull| unrepresented(pull, &represented, &include)),
    );
    result.diagnostics.extend(attempts::checks(
        pulls.iter().copied().filter(|pull| !pull.kill),
        completed.first().context("Missing reference")?,
        &result.events,
        &include,
    )?);
    wave_evidence(&mut result, pulls)?;
    Ok(result)
}

fn wave_evidence(result: &mut Common, pulls: &[&Pull]) -> Result<()> {
    result
        .events
        .iter_mut()
        .chain(
            result
                .branches
                .iter_mut()
                .flat_map(|branch| &mut branch.options)
                .flat_map(|option| &mut option.events),
        )
        .try_for_each(|event| annotate_wave(event, pulls))
}

// Retain instance counts without presenting them as hit counts or changing the standalone KDL.
fn annotate_wave(event: &mut Event, pulls: &[&Pull]) -> Result<()> {
    let mut waves = BTreeMap::new();
    for pull in pulls {
        let id = trace(pull);
        let Some(at) = event.samples.get(&id) else {
            continue;
        };
        let batch = pull
            .batches
            .binary_search_by_key(at, |batch| batch.at_ms)
            .map_err(|insertion| anyhow::anyhow!("Missing wave sample at insertion {insertion}"))?;
        let frame = pull.batches.get(batch).context("Missing wave batch")?;
        let count = frame
            .signals
            .iter()
            .filter(|signal| *signal == &event.signal)
            .count();
        waves.insert(id, json!({"batch":batch,"instances":count}));
    }
    if waves
        .values()
        .any(|wave| wave["instances"].as_u64().is_some_and(|count| count > 1))
    {
        if event.evidence.is_null() {
            event.evidence = json!({"method":"exact-timestamp-wave-v1"});
        }
        event
            .evidence
            .as_object_mut()
            .context("Invalid wave evidence")?
            .insert("waves".into(), json!(waves));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn signal(id: u8) -> Signal {
        Signal {
            kind: "cast".into(),
            identifier: super::super::Identifier::Ability(i64::from(id)),
            source: Some("Boss".into()),
            target: None,
        }
    }

    fn observations<'a>(signals: &'a [Signal], times: &[i64]) -> Vec<Observation<'a>> {
        signals
            .iter()
            .zip(times)
            .enumerate()
            .map(|(batch, (signal, at))| Observation {
                signal,
                batch,
                at: *at,
                ordinal: 1,
            })
            .collect()
    }

    #[rstest::rstest]
    #[case(vec![0,80,160],vec![3])]
    #[case(vec![0,100,201],vec![2,1])]
    #[case(vec![0,0,101],vec![2,1])]
    fn uncertainty_does_not_invent_order_in_a_chain(
        #[case] times: Vec<i64>,
        #[case] sizes: Vec<usize>,
    ) {
        let signals = vec![signal(1), signal(2), signal(3)];
        let result = groups(observations(&signals, &times));
        assert_eq!(
            result
                .iter()
                .map(|group| group.observations.len())
                .collect::<Vec<_>>(),
            sizes
        );
        assert_eq!(
            result
                .iter()
                .flat_map(|group| &group.observations)
                .map(|row| row.at)
                .collect::<Vec<_>>(),
            times
        );
    }

    proptest! {
        #[test]
        fn reordering_unresolved_observations_preserves_keys_and_multiplicity(ids in prop::collection::vec(0_u8..4,1..12)) {
            let signals:Vec<_>=ids.iter().copied().map(signal).collect();
            let reversed:Vec<_>=signals.iter().rev().cloned().collect();
            let times:Vec<_>=(0..signals.len()).map(|i| i as i64*50).collect();
            let forward=groups(observations(&signals,&times));
            let backward=groups(observations(&reversed,&times));
            let first=forward.first().unwrap();
            prop_assert_eq!(&first.key,&backward.first().unwrap().key);
            for value in &signals {
                let count=signals.iter().filter(|other|*other==value).count();
                prop_assert_eq!(first.unique(value).is_some(),count==1);
            }
        }
    }
}

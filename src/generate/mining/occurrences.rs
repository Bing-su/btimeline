use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Pull, Signal, alignment};

mod attempts;
mod branches;
mod entry;
mod fragments;
mod loops;
mod tail;
pub(in crate::generate) use loops::{LoopDiscovery, LoopModel};

pub(super) fn loop_models(pulls: &[&Pull], boss_only: bool) -> Result<LoopDiscovery> {
    loops::discover(pulls, boss_only)
}

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
    entities: std::collections::BTreeSet<(i64, Option<i64>)>,
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
        if self.key.iter().filter(|value| **value == signal).count() != 1 {
            return None;
        }
        self.observations.iter().find(|row| row.signal == signal)
    }
}

fn trace(pull: &Pull) -> String {
    format!("{}/{}", pull.report, pull.fight)
}

enum Boundary {
    Ordered,
    Display,
}

fn interval<'a>(
    pull: &'a Pull,
    left: i64,
    right: Option<i64>,
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
    boundary: Boundary,
) -> Result<Vec<Group<'a>>, &'static str> {
    span(
        pull,
        left,
        right,
        source,
        &|signal| &signal.source == source && include(signal),
        boundary,
    )
}

// Separate the time scope from caster selection, e.g. one combat span may contain several NPCs.
fn span<'a>(
    pull: &'a Pull,
    left: i64,
    right: Option<i64>,
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
    boundary: Boundary,
) -> Result<Vec<Group<'a>>, &'static str> {
    let end = right.unwrap_or_else(|| pull.scope_end(source));
    let start = source
        .as_ref()
        .and_then(|source| pull.boss_scopes.get(source))
        .map_or(0, |(start, _)| *start)
        .max(left);
    if pull
        .phase_transitions
        .iter()
        // A phase at the combat/boss scope start is initial state, not an interior transition.
        .any(|phase| start < phase.relative_ms && phase.relative_ms < end)
    {
        return Err("phase boundary");
    }
    let mut counts = BTreeMap::new();
    let mut result = Vec::new();
    for (batch, frame) in pull.batches.iter().enumerate() {
        let selected: Vec<_> = frame
            .waves()
            .into_iter()
            .filter(|signal| include(signal))
            .collect();
        for signal in &selected {
            *counts.entry(*signal).or_insert(0) += 1;
        }
        if frame.at_ms < left || frame.at_ms > end || selected.is_empty() {
            continue;
        }
        if frame.at_ms == left || Some(frame.at_ms) == right {
            if selected.len() > 1 && matches!(boundary, Boundary::Ordered) {
                return Err("unordered boundary");
            }
            continue;
        }
        // Display-only rows need no order relative to an anchor, e.g. A@0 and B@45ms.
        if matches!(boundary, Boundary::Ordered)
            && (frame.at_ms - left <= super::common::ORDER_RESOLUTION_MS
                || right
                    .is_some_and(|right| right - frame.at_ms <= super::common::ORDER_RESOLUTION_MS))
        {
            return Err("unordered boundary");
        }
        for signal in selected {
            result.push(Observation {
                signal,
                batch,
                at: frame.at_ms,
                ordinal: *counts.get(signal).ok_or("missing ordinal")?,
                entities: entities(pull, frame, signal),
            });
        }
    }
    Ok(groups(result))
}

// Keep instance provenance so separate packets cannot hide a repeated cast by the same entity.
fn entities(
    pull: &Pull,
    frame: &super::Batch,
    signal: &Signal,
) -> std::collections::BTreeSet<(i64, Option<i64>)> {
    let super::Identifier::Ability(ability) = signal.identifier else {
        return Default::default();
    };
    frame
        .indices
        .iter()
        .filter_map(|index| pull.events.get(*index))
        .filter_map(|event| {
            let actor = event.source_id?;
            (event.kind == signal.kind
                && event.ability_game_id == Some(ability)
                && pull.actors.get(&actor) == signal.source.as_ref())
            .then_some((actor, event.source_instance))
        })
        .collect()
}

fn unrepresented(pull: &Pull, events: &[&Event], include: &impl Fn(&Signal) -> bool) -> Value {
    let id = trace(pull);
    let rows: Vec<_> = pull
        .batches
        .iter()
        .enumerate()
        .flat_map(|(batch, frame)| {
            let id = id.as_str();
            frame
                .signals
                .iter()
                .filter(|signal| include(signal))
                .filter(move |signal| {
                    !events.iter().any(|event| {
                        &event.signal == *signal && covers(event, id, frame.at_ms, batch)
                    })
                })
                .map(move |signal| json!({"batch":batch,"activity":signal}))
        })
        .collect();
    json!({"trace":id,"unrepresented":rows,"reason":"no forced correspondence or supported branch"})
}

fn covers(event: &Event, trace: &str, at: i64, batch: usize) -> bool {
    event.samples.get(trace) == Some(&at)
        || event
            .evidence
            .get("fragments")
            .and_then(|value| value.get(trace))
            .and_then(|value| value.get("batches"))
            .and_then(Value::as_array)
            .is_some_and(|values| {
                values
                    .iter()
                    .any(|value| value.as_u64() == Some(batch as u64))
            })
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
    for signal in &reference.key {
        let Some(observation) = reference.unique(signal) else {
            continue;
        };
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
        let mut evidence = json!({"method":"anchor-constrained-multiset-lcs-v2","bounds":bounds,"reference":trace(pulls.first().context("Missing reference")?),"batches":batches,"groups":blocks,"sync":"omitted: repeated signal identity"});
        if let Some(fragments) = fragments::evidence(pulls, groups, observation.signal) {
            let fields = evidence
                .as_object_mut()
                .context("Invalid alignment evidence")?;
            fields.insert("fragments".into(), fragments);
            fields.insert("fragment_policy".into(), "disjoint-instance-peer-v1".into());
        }
        result.push(Event {
            signal: observation.signal.clone(),
            samples,
            occurrence: Some(observation.ordinal),
            evidence,
        });
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

fn bounded(
    pulls: &[&Pull],
    anchors: &[(&Signal, &BTreeMap<String, i64>)],
    include: &impl Fn(&Signal) -> bool,
    result: &mut Common,
) -> Result<()> {
    for window in anchors.windows(2) {
        let [(left, a), (right, b)] = window else {
            continue;
        };
        let bounds = json!({"left":left,"right":right,"boundary_policy":"display-interior-v1"});
        let rows = pulls
            .iter()
            .map(|pull| {
                let id = trace(pull);
                let start = a.get(&id).ok_or((id.clone(), "missing bound"))?;
                let end = b.get(&id).ok_or((id.clone(), "missing bound"))?;
                interval(
                    pull,
                    *start,
                    Some(*end),
                    &left.source,
                    include,
                    Boundary::Display,
                )
                .map_err(|reason| (id, reason))
            })
            .collect::<Result<Vec<_>, _>>();
        match rows {
            Ok(mut rows) => {
                fragments::normalize(&mut rows)?;
                expand_interval(
                    pulls,
                    &rows.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                    bounds,
                    result,
                )?;
            }
            Err((trace, reason)) => result
                .diagnostics
                .push(json!({"bounds":bounds,"trace":trace,"status":"unresolved","reason":reason})),
        }
    }
    Ok(())
}

// A boss's own first cast bounds its opening without mixing travel or preceding bosses into its clock.
fn opening_start(
    pull: &Pull,
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
    raid: bool,
) -> Result<i64, &'static str> {
    if raid {
        return Ok(-1);
    }
    pull.batches
        .iter()
        .find(|batch| {
            batch
                .waves()
                .iter()
                .any(|signal| &signal.source == source && include(signal))
        })
        .map(|batch| batch.at_ms - 1)
        .ok_or("missing boss opening")
}

// Recover repeated openings before the first unique anchor without adding repeated-ID syncs.
fn opening(
    pulls: &[&Pull],
    anchors: &[(&Signal, &BTreeMap<String, i64>)],
    include: &impl Fn(&Signal) -> bool,
    raid: bool,
    result: &mut Common,
) -> Result<()> {
    let Some((right, ends)) = anchors.first() else {
        return Ok(());
    };
    if result.branches.iter().any(|branch| &branch.join == *right) {
        return Ok(());
    }
    let bounds = json!({"left":null,"right":right,"origin":if raid {"combat-start"} else {"boss-cast"},"boundary_policy":"display-interior-v1"});
    let rows = pulls
        .iter()
        .map(|pull| {
            let id = trace(pull);
            let end = ends.get(&id).ok_or((id.clone(), "missing bound"))?;
            let start = opening_start(pull, &right.source, include, raid)
                .map_err(|reason| (id.clone(), reason))?;
            interval(
                pull,
                start,
                Some(*end),
                &right.source,
                include,
                Boundary::Display,
            )
            .map_err(|reason| (id, reason))
        })
        .collect::<Result<Vec<_>, _>>();
    match rows {
        Ok(mut rows) => {
            fragments::normalize(&mut rows)?;
            expand_interval(
                pulls,
                &rows.iter().map(Vec::as_slice).collect::<Vec<_>>(),
                bounds,
                result,
            )?;
        }
        Err((trace, reason)) => result
            .diagnostics
            .push(json!({"bounds":bounds,"trace":trace,"status":"unresolved","reason":reason})),
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
    for anchors in scopes.values_mut() {
        anchors.sort_by_key(|(_, samples)| samples.get(&reference));
        tail::expand(pulls, anchors, &include, raid, &mut result)?;
        bounded(&completed, anchors, &include, &mut result)?;
    }
    branches::discover(
        &completed,
        &all_unique,
        &unique,
        &include,
        raid,
        &mut result,
    )?;
    for anchors in scopes.into_values() {
        opening(&completed, &anchors, &include, raid, &mut result)?;
    }
    if !raid {
        entry::expand(&completed, &all_unique, &include, &mut result)?;
    }
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
        pulls
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
                entities: Default::default(),
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

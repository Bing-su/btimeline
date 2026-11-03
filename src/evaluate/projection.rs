//! Explicit authoring assumptions; encounter IDs, names and display summaries stay in data files.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fflogs::model::{Actor, CollectedLog, Event};
use crate::timeline::{CastEntry, MAX_MS, Observation, Recording};

#[derive(Debug)]
pub(super) struct InputExclusion {
    pub message: &'static str,
    pub evidence: Value,
}
impl std::fmt::Display for InputExclusion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for InputExclusion {}
pub(super) fn exclude(message: &'static str, evidence: Value) -> anyhow::Error {
    InputExclusion { message, evidence }.into()
}

#[derive(Debug)]
pub(super) struct UnverifiedScope(pub &'static str, pub Value);

impl std::fmt::Display for UnverifiedScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for UnverifiedScope {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    version: u32,
    observations: ObservationScope,
    segments: Vec<SegmentRule>,
    pub not_checked: Vec<String>,
    #[serde(skip)]
    pub(super) inferred: bool,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ObservationScope {
    AllCasts,
    ScopedCasts,
    ScopedCastsAndStarts,
    #[serde(skip)]
    AllCastsAndStarts,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Completion {
    Defeat,
    FightEnd,
    FirstDefeat,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentRule {
    phase: String,
    encounter: Option<i64>,
    source: String,
    #[serde(default)]
    other_sources: Vec<String>,
    entry: Vec<String>,
    #[serde(skip)]
    entry_kind: Option<&'static str>,
    #[serde(skip)]
    entry_from: Option<String>,
    completion: Completion,
    max_duration_ms: Option<i64>,
    #[serde(default)]
    require_main_actor: bool,
    #[serde(default)]
    reject_health_reset: bool,
    targets: BTreeMap<String, TargetRule>,
    #[serde(default)]
    ignored: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetRule {
    name: String,
    events: Vec<String>,
    #[serde(default)]
    equivalents: Vec<Vec<String>>,
    source: Option<String>,
    group: Option<String>,
    lead_ms: Option<i64>,
}
pub(super) struct Target {
    pub raw_index: usize,
    pub at_ms: i64,
    pub id: String,
    pub name: String,
    pub lead_ms: Option<i64>,
    pub event_ids: Vec<String>,
    pub equivalents: Vec<Vec<String>>,
}
pub(super) struct Segment {
    pub phase: String,
    pub entry_ms: i64,
    pub end_ms: i64,
    pub entry_event: usize,
    pub completion_event: usize,
    pub targets: Vec<Target>,
    pub audit: super::audit::Audit,
}
pub(super) struct Projection {
    pub origin_timestamp_ms: i64,
    pub recording: Recording,
    pub bosses: Vec<Segment>,
}

impl Profile {
    pub(super) fn infer(model: &crate::timeline::Timeline) -> Option<Self> {
        let segments = model
            .cast_entries()
            .map(|entry| SegmentRule::infer(model, entry))
            .collect::<Vec<_>>();
        (!segments.is_empty()).then(|| Self {
            version: 1,
            observations: ObservationScope::AllCastsAndStarts,
            segments,
            not_checked: vec!["Displays without an unambiguous same-time boss Ability sync".into()],
            inferred: true,
        })
    }
}

impl SegmentRule {
    fn infer(model: &crate::timeline::Timeline, entry: CastEntry<'_>) -> Self {
        let CastEntry {
            phase,
            source,
            from,
            raw_kind,
            ids,
        } = entry;
        let mut targets = BTreeMap::<String, TargetRule>::new();
        let visible = || {
            model.catalog.events.values().filter(|definition| {
                definition.event.visible && definition.owner.phase.as_deref() == Some(phase)
            })
        };
        let unbound_names = visible()
            .filter(|definition| definition.cast.is_none())
            .map(|definition| definition.event.name.as_str())
            .collect::<BTreeSet<_>>();
        // A known binding cannot distinguish another occurrence displayed under the same unbound name.
        let ambiguous_ids = visible()
            .filter(|definition| unbound_names.contains(definition.event.name.as_str()))
            .filter_map(|definition| {
                definition
                    .cast
                    .as_ref()
                    .filter(|(_, actor)| actor == source)
            })
            .map(|(id, _)| id.as_str())
            .collect::<BTreeSet<_>>();
        for definition in visible() {
            let Some((id, _)) = definition
                .cast
                .as_ref()
                .filter(|(_, actor)| actor == source)
            else {
                continue;
            };
            if ambiguous_ids.contains(id.as_str()) {
                continue;
            }
            let target = targets.entry(id.clone()).or_insert_with(|| TargetRule {
                name: definition.event.name.clone(),
                events: Vec::new(),
                equivalents: Vec::new(),
                source: None,
                group: None,
                lead_ms: None,
            });
            target.events.push(definition.event.id.clone());
        }
        Self {
            phase: phase.into(),
            encounter: None,
            source: source.into(),
            other_sources: Vec::new(),
            entry: ids,
            entry_kind: Some(raw_kind),
            entry_from: from.map(str::to_owned),
            completion: Completion::FightEnd,
            max_duration_ms: None,
            require_main_actor: true,
            reject_health_reset: true,
            targets,
            ignored: BTreeMap::new(),
        }
    }
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && !self.segments.is_empty(),
            "Unsupported or empty projection profile"
        );
        let mut phases = std::collections::BTreeSet::new();
        for segment in &self.segments {
            ensure!(
                !segment.phase.is_empty() && phases.insert(&segment.phase),
                "Empty or duplicate segment phase"
            );
            ensure!(
                !segment.source.is_empty()
                    && !segment.entry.is_empty()
                    && (self.inferred || !segment.targets.is_empty()),
                "Segment requires source, entry IDs and targets"
            );
            ensure!(
                segment
                    .max_duration_ms
                    .is_none_or(|v| (1..=MAX_MS).contains(&v)),
                "Invalid segment duration"
            );
            ensure!(
                !segment.reject_health_reset || segment.require_main_actor,
                "Health-reset checks require an unambiguous main actor"
            );
            for id in segment
                .entry
                .iter()
                .chain(segment.targets.keys())
                .chain(segment.ignored.keys())
            {
                ensure!(
                    !id.is_empty()
                        && id
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_lowercase()),
                    "Ability IDs must be uppercase hexadecimal"
                );
            }
            for (id, reason) in &segment.ignored {
                ensure!(
                    !reason.trim().is_empty() && !segment.targets.contains_key(id),
                    "Ignored IDs require a reason and must not overlap targets"
                );
            }
            for target in segment.targets.values() {
                target.validate()?;
            }
        }
        Ok(())
    }

    pub fn validate_bindings(&self, model: &crate::timeline::Timeline) -> Result<()> {
        self.segments
            .iter()
            .try_for_each(|segment| segment.validate_bindings(model))
    }
}
pub(super) fn ability(event: &Event) -> Result<String> {
    let id = event
        .ability_game_id
        .filter(|id| *id >= 0)
        .context("Cast missing nonnegative abilityGameID")?;
    Ok(format!("{id:X}"))
}
fn hp(event: &Event, field: &str) -> Option<f64> {
    event.extra.get("targetResources")?.get(field)?.as_f64()
}
fn is_defeat(event: &Event) -> bool {
    event.kind == "damage" && hp(event, "hitPoints") == Some(0.0)
}

fn main_actors<'a>(log: &'a CollectedLog, rule: &SegmentRule) -> Result<Vec<&'a Actor>> {
    if !rule.require_main_actor {
        return Ok(Vec::new());
    }
    std::iter::once(&rule.source)
        .chain(&rule.other_sources)
        .map(|name| {
            let mut actors = log
                .report
                .master_data
                .actors
                .iter()
                .filter(|a| a.name == *name && a.sub_type == "Boss");
            let actor = actors
                .next()
                .filter(|_| actors.next().is_none())
                .ok_or_else(|| exclude("Unclear main boss actor", json!({"source":name})))?;
            Ok(actor)
        })
        .collect()
}
fn check_health(log: &CollectedLog, actors: &[&Actor], start: i64, end: i64) -> Result<()> {
    for actor in actors {
        let mut lowest = 1.0_f64;
        for (raw_index, event) in log.events.iter().enumerate().filter(|(_, e)| {
            e.timestamp >= start
                && e.timestamp <= end
                && e.kind == "damage"
                && e.target_id == Some(actor.id)
        }) {
            let Some((current, max)) = hp(event, "hitPoints")
                .zip(hp(event, "maxHitPoints"))
                .filter(|(_, max)| *max > 0.0)
            else {
                continue;
            };
            let ratio = current / max;
            if lowest < 0.75 && ratio > 0.9 {
                return Err(exclude(
                    "Possible attempt reset or large boss heal",
                    json!({"source":actor.name,"raw_index":raw_index}),
                ));
            }
            lowest = lowest.min(ratio);
        }
    }
    Ok(())
}
fn find_entries<'a>(
    log: &'a CollectedLog,
    rule: &'a SegmentRule,
    actors: &'a BTreeMap<i64, &Actor>,
) -> impl Iterator<Item = (usize, &'a Event)> + 'a {
    log.events.iter().enumerate().filter(move |(_, event)| {
        event.kind == rule.entry_kind.unwrap_or("cast")
            && event
                .source_id
                .and_then(|id| actors.get(&id))
                .is_some_and(|actor| actor.name == rule.source)
            && event
                .ability_game_id
                .is_some_and(|id| rule.entry.contains(&format!("{id:X}")))
    })
}

enum Boundary {
    DefeatBefore(i64),
    TransitionAt(i64),
}

type PhaseEntry<'a> = (&'a SegmentRule, usize, &'a Event);

fn phase_ambiguity(message: &'static str, entries: &[PhaseEntry<'_>]) -> anyhow::Error {
    let evidence = entries
        .iter()
        .map(|(rule, index, _)| json!({"phase":rule.phase,"source":rule.source,"raw_index":index}))
        .collect::<Vec<_>>();
    UnverifiedScope(message, json!({"entry_candidates":evidence})).into()
}

fn follow_transitions<'a>(
    log: &'a CollectedLog,
    rules: &'a [SegmentRule],
    actors: &'a BTreeMap<i64, &Actor>,
    root: PhaseEntry<'a>,
    limit: usize,
) -> Result<Vec<PhaseEntry<'a>>> {
    let mut route = Vec::new();
    let mut current = root;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(&current.0.phase) {
            return Err(phase_ambiguity(
                "Repeated phase in inferred sequence",
                &[current],
            ));
        }
        route.push(current);
        let mut choices = rules
            .iter()
            .filter(|rule| rule.entry_from.as_deref() == Some(&current.0.phase))
            .filter_map(|rule| {
                find_entries(log, rule, actors)
                    .take_while(|(index, _)| *index < limit)
                    .find(|(index, _)| *index > current.1)
                    .map(|(index, event)| (rule, index, event))
            })
            .collect::<Vec<_>>();
        choices.sort_by_key(|(_, index, _)| *index);
        if let [first, second, ..] = choices.as_slice()
            && first.2.timestamp == second.2.timestamp
        {
            return Err(phase_ambiguity(
                "Multiple phase transitions at the same time",
                &choices,
            ));
        }
        let next = choices.first().copied();
        if let Some(next) = next
            && next.2.timestamp == current.2.timestamp
        {
            return Err(phase_ambiguity(
                "Phase transition shares the prior entry timestamp",
                &[current, next],
            ));
        }
        if let Some(next) = next
            && next.0.source != current.0.source
        {
            return Err(phase_ambiguity(
                "Transition changes source and needs an explicit scope",
                &[current, next],
            ));
        }
        let boundary = next.map_or(limit, |(_, index, _)| index);
        // Reuse in a later phase is valid. Repetition before leaving this phase is ambiguous.
        if current.0.entry_from.is_some()
            && let Some((index, event)) = find_entries(log, current.0, actors)
                .take_while(|(index, _)| *index < boundary)
                .find(|(index, _)| *index > current.1)
        {
            return Err(phase_ambiguity(
                "Repeated transition before the next phase",
                &[current, (current.0, index, event)],
            ));
        }
        let Some(next) = next else { break };
        current = next;
    }
    Ok(route)
}

fn inferred_segments(
    log: &CollectedLog,
    rules: &[SegmentRule],
    actors: &BTreeMap<i64, &Actor>,
    origin: i64,
) -> Result<Vec<Segment>> {
    let mut roots = rules
        .iter()
        .filter(|rule| rule.entry_from.is_none())
        .filter_map(|rule| {
            find_entries(log, rule, actors)
                .next()
                .map(|(index, event)| (rule, index, event))
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|(_, index, _)| *index);
    let mut sources = BTreeSet::new();
    if roots.is_empty()
        || !roots
            .iter()
            .all(|(rule, _, _)| sources.insert(&rule.source))
    {
        return Err(phase_ambiguity(
            "Observed phase entries do not establish distinct starting scopes",
            &roots,
        ));
    }
    let limits = roots
        .iter()
        .skip(1)
        .map(|(_, index, _)| *index)
        .chain(std::iter::once(log.events.len()));
    let mut candidates = Vec::new();
    for (root, limit) in roots.iter().copied().zip(limits) {
        candidates.extend(follow_transitions(log, rules, actors, root, limit)?);
    }
    // Same-boss transitions use the explicit entry; switching bosses still requires defeat.
    let next_entries = candidates
        .iter()
        .skip(1)
        .map(|(rule, _, event)| {
            Some(if rule.entry_from.is_some() {
                Boundary::TransitionAt(event.timestamp)
            } else {
                Boundary::DefeatBefore(event.timestamp)
            })
        })
        .chain(std::iter::once(None));
    candidates
        .iter()
        .zip(next_entries)
        .map(|((rule, index, event), next_entry)| {
            project_segment(
                log,
                rule,
                actors,
                origin,
                true,
                next_entry,
                Some((*index, *event)),
            )
        })
        .collect()
}

fn project_segment(
    log: &CollectedLog,
    rule: &SegmentRule,
    actors: &BTreeMap<i64, &Actor>,
    origin: i64,
    inferred: bool,
    next_entry: Option<Boundary>,
    inferred_entry: Option<(usize, &Event)>,
) -> Result<Segment> {
    let main = main_actors(log, rule)?;
    let source = |id: Option<i64>| id.and_then(|id| actors.get(&id)).map(|a| a.name.as_str());
    let entry = inferred_entry
        .or_else(|| find_entries(log, rule, actors).next())
        .ok_or_else(|| exclude("Missing boss entry", json!({"source":rule.source})))?;
    let (entry_event, start) = (entry.0, entry.1.timestamp);
    let defeat = log.events.iter().enumerate().find(|(_, e)| {
        e.timestamp >= start
            && is_defeat(e)
            && if rule.require_main_actor {
                main.iter().any(|a| Some(a.id) == e.target_id)
            } else {
                source(e.target_id) == Some(&rule.source)
            }
    });
    let fight = log.report.fights.first().context("Missing fight")?;
    let end = if let Some(Boundary::TransitionAt(next)) = next_entry {
        // The entry millisecond belongs to the new phase.
        next - 1
    } else if let Some(Boundary::DefeatBefore(next)) = next_entry {
        defeat.filter(|(_, event)| event.timestamp < next && start < next)
            .map(|(_, event)| event.timestamp)
            .ok_or_else(|| UnverifiedScope(
                "Missing observed phase defeat before the next entry",
                json!({"entry_candidates":[{"phase":rule.phase,"source":rule.source,"raw_index":entry_event}]}),
            ))?
    } else {
        match rule.completion {
            Completion::Defeat => defeat.map(|(_, e)| e.timestamp).ok_or_else(|| {
                exclude(
                    "Missing observed boss completion",
                    json!({"source":rule.source,"entry_event":entry_event}),
                )
            })?,
            Completion::FightEnd => fight.end_time,
            Completion::FirstDefeat => defeat.map_or(fight.end_time, |(_, e)| e.timestamp),
        }
    };
    let end = rule
        .max_duration_ms
        .map(|limit| start.checked_add(limit).context("Scope end overflow"))
        .transpose()?
        .map_or(end, |limit| end.min(limit));
    ensure!(end >= start, "Invalid scope end");
    if rule.reject_health_reset {
        check_health(log, &main, start, end)?;
    }
    let completion_event = if matches!(rule.completion, Completion::Defeat) {
        defeat.context("Missing defeat")?.0
    } else {
        log.events
            .iter()
            .rposition(|e| e.timestamp <= end)
            .context("Empty scope")?
    };
    let mut last = BTreeMap::<(bool, &str), i64>::new();
    let mut targets = Vec::new();
    for (raw_index, event) in log
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == "cast" && e.timestamp >= start && e.timestamp <= end)
    {
        // Entry opens the scope and cannot establish an advance warning.
        if inferred && raw_index == entry_event {
            continue;
        }
        let id = ability(event)?;
        let Some((id_key, target)) = rule.targets.get_key_value(&id) else {
            continue;
        };
        let expected = target.source.as_deref().unwrap_or(&rule.source);
        if source(event.source_id) != Some(expected) {
            continue;
        }
        // Alias packets share only explicitly declared groups, e.g. C3FF and C400 form one Beam wave.
        // Keep explicit groups separate from implicit ability keys, e.g. group="B528" is not ID B528.
        let group = (
            target.group.is_some(),
            target.group.as_deref().unwrap_or(id_key),
        );
        if last
            .get(&group)
            .is_some_and(|before| event.timestamp - before <= 100)
        {
            continue;
        }
        last.insert(group, event.timestamp);
        targets.push(target.project(raw_index, event.timestamp - origin, id));
    }
    ensure!(
        inferred || !targets.is_empty(),
        "No evaluation targets in declared scope"
    );
    let audit = audit_segment(log, rule, start..=end, origin)?;
    Ok(Segment {
        phase: rule.phase.clone(),
        entry_ms: start - origin,
        end_ms: end - origin,
        entry_event,
        completion_event,
        targets,
        audit,
    })
}

fn audit_segment(
    log: &CollectedLog,
    rule: &SegmentRule,
    range: std::ops::RangeInclusive<i64>,
    origin: i64,
) -> Result<super::audit::Audit> {
    let sources = std::iter::once(rule.source.as_str())
        .chain(rule.other_sources.iter().map(String::as_str))
        .chain(
            rule.targets
                .values()
                .filter_map(|target| target.source.as_deref()),
        )
        .collect();
    super::audit::collect(
        log,
        range,
        origin,
        &sources,
        |source, id| {
            rule.targets
                .get(id)
                .is_some_and(|target| source == target.source.as_deref().unwrap_or(&rule.source))
        },
        &rule.ignored,
    )
}

pub(super) fn project(log: &CollectedLog, profile: &Profile) -> Result<Projection> {
    let origin = log.events.first().context("Empty events")?.timestamp;
    let actors = log
        .report
        .master_data
        .actors
        .iter()
        .map(|a| (a.id, a))
        .collect::<BTreeMap<_, _>>();
    let encounter = log
        .report
        .fights
        .first()
        .context("Missing fight")?
        .encounter_id;
    let bosses = if profile.inferred {
        inferred_segments(log, &profile.segments, &actors, origin)?
    } else {
        profile
            .segments
            .iter()
            .filter(|r| r.encounter.is_none_or(|id| id == encounter))
            .map(|rule| project_segment(log, rule, &actors, origin, false, None, None))
            .collect::<Result<Vec<_>>>()?
    };
    if bosses.is_empty() {
        return Err(exclude(
            "Unsupported encounter",
            json!({"encounter":encounter}),
        ));
    }
    if bosses.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|(a, b)| b.entry_ms <= a.end_ms)
    }) {
        return Err(exclude("Overlapping boss attempts", json!({})));
    }
    let end_ms = bosses.last().context("Missing scope")?.end_ms;
    let mut observations = Vec::new();
    for (raw_index, event) in log.events.iter().enumerate() {
        let at_ms = event.timestamp - origin;
        if event.kind != "cast"
            && !(event.kind == "begincast"
                && matches!(
                    profile.observations,
                    ObservationScope::ScopedCastsAndStarts | ObservationScope::AllCastsAndStarts
                ))
        {
            continue;
        }
        if !matches!(
            profile.observations,
            ObservationScope::AllCasts | ObservationScope::AllCastsAndStarts
        ) && !bosses
            .iter()
            .any(|b| at_ms >= b.entry_ms && at_ms <= b.end_ms)
        {
            continue;
        }
        let source = event
            .source_id
            .and_then(|id| actors.get(&id))
            .map(|a| a.name.as_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                exclude(
                    "Unknown cast source",
                    json!({"raw_index":raw_index,"source_id":event.source_id}),
                )
            })?;
        if at_ms <= end_ms {
            observations.push(Observation {
                at_ms,
                kind: if event.kind == "cast" {
                    "Ability"
                } else {
                    "StartsUsing"
                }
                .into(),
                fields: BTreeMap::from([
                    ("id".into(), ability(event)?),
                    ("source".into(), source.into()),
                ]),
                source_event_index: Some(raw_index),
            });
        }
    }
    Ok(Projection {
        origin_timestamp_ms: origin,
        recording: Recording {
            version: 1,
            end_ms,
            observations,
        },
        bosses,
    })
}

impl TargetRule {
    fn validate(&self) -> Result<()> {
        let unique = self
            .events
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            !self.name.is_empty()
                && !self.events.is_empty()
                && unique.len() == self.events.len()
                && self.lead_ms.is_none_or(|v| (1..=MAX_MS).contains(&v)),
            "Invalid target name, event IDs or forecast lead"
        );
        let mut seen = std::collections::BTreeSet::new();
        for group in &self.equivalents {
            ensure!(
                group.len() >= 2
                    && group
                        .iter()
                        .all(|event| self.events.contains(event) && seen.insert(event)),
                "Equivalence groups must be disjoint subsets of declared event IDs"
            );
        }
        Ok(())
    }

    fn project(&self, raw_index: usize, at_ms: i64, id: String) -> Target {
        Target {
            raw_index,
            at_ms,
            id,
            name: self.name.clone(),
            lead_ms: self.lead_ms,
            event_ids: self
                .events
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            equivalents: self.equivalents.clone(),
        }
    }
}
impl SegmentRule {
    fn validate_bindings(&self, model: &crate::timeline::Timeline) -> Result<()> {
        let definitions = model
            .catalog
            .events
            .values()
            .map(|event| (event.event.id.as_str(), event))
            .collect::<BTreeMap<_, _>>();
        let mut sets = Vec::<std::collections::BTreeSet<&str>>::new();
        let mut groups = BTreeMap::new();
        for (id, target) in &self.targets {
            let events = target
                .events
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            for event in &events {
                let definition = definitions
                    .get(event)
                    .with_context(|| format!("Unknown event binding {event} for {id}"))?;
                ensure!(
                    definition.event.visible
                        && definition.owner.phase.as_deref() == Some(&self.phase),
                    "Binding {event} must be visible and belong to phase {}",
                    self.phase
                );
            }
            ensure!(
                sets.iter()
                    .all(|previous| previous == &events || previous.is_disjoint(&events)),
                "Overlapping event bindings must declare the same candidate set in {}",
                self.phase
            );
            if let Some(group) = &target.group
                && let Some(previous) = groups.insert(group, (&target.name, events.clone()))
            {
                ensure!(
                    previous == (&target.name, events.clone()),
                    "Collapsed group {group} must share name and event bindings"
                );
            }
            sets.push(events);
        }
        Ok(())
    }
}

//! Explicit authoring assumptions; encounter IDs, names and display summaries stay in data files.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::fflogs::model::{Actor, CollectedLog, Event};
use crate::timeline::{CastEntry, MAX_MS, Observation, Record, Recording, Timeline};

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
    #[serde(default)]
    source_aliases: BTreeMap<String, String>,
    #[serde(default)]
    continuations: BTreeSet<String>,
    pub not_checked: Vec<String>,
    #[serde(default, rename = "infer_segments")]
    pub(super) inferred: bool,
}
#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ObservationScope {
    AllCasts,
    ScopedCasts,
    ScopedCastsAndStarts,
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
    entry_from: Option<String>,
    #[serde(skip)]
    inferred_entries: Vec<(Option<String>, String)>,
    completion: Completion,
    max_duration_ms: Option<i64>,
    #[serde(default)]
    require_main_actor: bool,
    #[serde(default)]
    reject_health_reset: bool,
    #[serde(deserialize_with = "target_rules")]
    targets: BTreeMap<String, Vec<TargetRule>>,
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
    #[serde(default)]
    reference: Reference,
    #[serde(default = "default_group_window")]
    group_window_ms: i64,
    window: Option<TargetWindow>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetWindow {
    anchor: String,
    occurrence: usize,
    duration_ms: i64,
}

fn target_rules<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Vec<TargetRule>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Rules {
        One(TargetRule),
        Many(Vec<TargetRule>),
    }
    BTreeMap::<String, Rules>::deserialize(deserializer).map(|rules| {
        rules
            .into_iter()
            .map(|(id, rules)| {
                (
                    id,
                    match rules {
                        Rules::One(rule) => vec![rule],
                        Rules::Many(rules) => rules,
                    },
                )
            })
            .collect()
    })
}

fn default_group_window() -> i64 {
    100
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Reference {
    #[default]
    EventStart,
    ActiveWindow,
}
pub(super) struct Target {
    pub raw_index: usize,
    pub raw_indices: Vec<usize>,
    pub at_ms: i64,
    pub id: String,
    pub name: String,
    pub lead_ms: Option<i64>,
    pub event_ids: Vec<String>,
    pub equivalents: Vec<Vec<String>>,
    pub reference: Reference,
}

impl Target {
    pub fn timing_error(&self, at_ms: i64, duration_ms: i64) -> i64 {
        match self.reference {
            Reference::EventStart => (at_ms - self.at_ms).abs(),
            Reference::ActiveWindow => {
                if self.at_ms < at_ms {
                    at_ms - self.at_ms
                } else {
                    (self.at_ms - at_ms - duration_ms.max(1) + 1).max(0)
                }
            }
        }
    }
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
    pub(super) fn resolve(mut self, model: &Timeline) -> Result<Self> {
        if self.inferred {
            ensure!(
                self.segments.is_empty()
                    && self.observations == ObservationScope::AllCastsAndStarts,
                "An inference profile requires empty segments and all_casts_and_starts observations"
            );
            let inferred =
                Self::infer(model).context("No supported entries for inference profile")?;
            self.segments = inferred.segments;
            self.not_checked.extend(inferred.not_checked);
        }
        Ok(self)
    }

    pub(super) fn apply_source_aliases(&self, log: &mut CollectedLog) {
        // Normalize the projection after validating the original input and its hash.
        for actor in &mut log.report.master_data.actors {
            if let Some(source) = self.source_aliases.get(&actor.name) {
                actor.name.clone_from(source);
            }
        }
    }

    pub(super) fn infer(model: &crate::timeline::Timeline) -> Option<Self> {
        let segments = model
            .cast_entries()
            .map(|entry| SegmentRule::infer(model, entry))
            .collect::<Vec<_>>();
        (!segments.is_empty()).then(|| Self {
            version: 1,
            observations: ObservationScope::AllCastsAndStarts,
            segments,
            source_aliases: BTreeMap::new(),
            continuations: BTreeSet::new(),
            not_checked: vec!["Displays without an unambiguous same-time boss Ability sync".into()],
            inferred: true,
        })
    }
}

impl SegmentRule {
    fn target_at<'r, 's>(
        &'r self,
        id: &str,
        event: &Event,
        log: &CollectedLog,
        source: &impl Fn(Option<i64>) -> Option<&'s str>,
    ) -> Result<Option<(&'r str, &'r TargetRule)>> {
        let Some((key, rules)) = self.targets.get_key_value(id) else {
            return Ok(None);
        };
        let matching = rules
            .iter()
            .filter(|target| {
                source(event.source_id) == Some(target.source.as_deref().unwrap_or(&self.source))
                    && target.in_window(event, log, source, &self.source)
            })
            .collect::<Vec<_>>();
        ensure!(
            matching.len() <= 1,
            "Overlapping target windows for {id} at {}",
            event.timestamp
        );
        Ok(matching.first().map(|target| (key.as_str(), *target)))
    }

    fn infer(model: &crate::timeline::Timeline, entry: CastEntry<'_>) -> Self {
        let CastEntry {
            phase,
            source,
            raw_kind,
            entries,
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
                reference: Reference::EventStart,
                group_window_ms: default_group_window(),
                window: None,
            });
            target.events.push(definition.event.id.clone());
        }
        Self {
            phase: phase.into(),
            encounter: None,
            source: source.into(),
            other_sources: Vec::new(),
            entry: entries.iter().map(|(_, id)| id.clone()).collect(),
            entry_kind: Some(raw_kind),
            entry_from: None,
            inferred_entries: entries
                .into_iter()
                .map(|(from, id)| (from.map(str::to_owned), id))
                .collect(),
            completion: Completion::FightEnd,
            max_duration_ms: None,
            require_main_actor: true,
            reject_health_reset: true,
            targets: targets
                .into_iter()
                .map(|(id, target)| (id, vec![target]))
                .collect(),
            ignored: BTreeMap::new(),
        }
    }

    fn accepts_entry(&self, from: Option<&str>, id: Option<&str>) -> bool {
        // Keep the cast ID paired with its predecessor when a phase has multiple entrances.
        if self.inferred_entries.is_empty() {
            self.entry_from.as_deref() == from
                && id.is_none_or(|id| self.entry.iter().any(|entry| entry == id))
        } else {
            self.inferred_entries
                .iter()
                .any(|(guard, entry)| guard.as_deref() == from && id.is_none_or(|id| entry == id))
        }
    }
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && !self.segments.is_empty(),
            "Unsupported or empty projection profile"
        );
        ensure!(
            self.source_aliases.iter().all(|(raw, canonical)| {
                !raw.trim().is_empty()
                    && !canonical.trim().is_empty()
                    && !self.source_aliases.contains_key(canonical)
            }),
            "Source aliases must map nonempty names directly to canonical names"
        );
        ensure!(
            self.continuations.iter().all(|phase| {
                self.inferred
                    && self
                        .segments
                        .iter()
                        .any(|rule| rule.phase == *phase && rule.accepts_entry(None, None))
            }),
            "Continuations must name inferred phases with unguarded entries"
        );
        let mut phases = BTreeMap::new();
        for segment in &self.segments {
            let scope = (segment.source.as_str(), segment.encounter);
            if let Some(from) = &segment.entry_from
                && !self.inferred
            {
                ensure!(
                    phases.get(from.as_str()).copied() == Some(scope),
                    "entry_from must reference an earlier segment with the same encounter and source"
                );
            }
            ensure!(
                !segment.phase.is_empty() && phases.insert(segment.phase.as_str(), scope).is_none(),
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
            ensure!(
                segment.targets.values().all(|rules| !rules.is_empty()),
                "Target rules cannot be empty"
            );
            for target in segment.targets.values().flatten() {
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

fn main_actors<'a>(
    log: &'a CollectedLog,
    rule: &SegmentRule,
    transition_entry: Option<(usize, &Event)>,
) -> Result<Vec<&'a Actor>> {
    // An entry-only transition can identify its NPC by the observed instance.
    if let Some((raw_index, event)) = transition_entry {
        let actor = log
            .report
            .master_data
            .actors
            .iter()
            .find(|actor| Some(actor.id) == event.source_id && actor.kind == "NPC")
            .ok_or_else(|| {
                exclude(
                    "Entry-only phase requires an observed NPC",
                    json!({"source":rule.source,"raw_index":raw_index}),
                )
            })?;
        return Ok(vec![actor]);
    }
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
    from: Option<&'a str>,
) -> impl Iterator<Item = (usize, &'a Event)> + 'a {
    log.events.iter().enumerate().filter(move |(_, event)| {
        event.kind == rule.entry_kind.unwrap_or("cast")
            && event
                .source_id
                .and_then(|id| actors.get(&id))
                .is_some_and(|actor| actor.name == rule.source)
            && event
                .ability_game_id
                .is_some_and(|id| rule.accepts_entry(from, Some(&format!("{id:X}"))))
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
    replay_choice: &mut impl FnMut(&[PhaseEntry<'a>]) -> Result<Option<usize>>,
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
            .filter(|rule| {
                rule.accepts_entry(Some(&current.0.phase), None)
                    && rule.encounter == current.0.encounter
            })
            .flat_map(|rule| {
                find_entries(log, rule, actors, Some(&current.0.phase))
                    .take_while(|(index, _)| *index < limit)
                    .filter(|(index, _)| *index > current.1)
                    .map(move |(index, event)| (rule, index, event))
            })
            .collect::<Vec<_>>();
        choices.sort_by_key(|(_, index, _)| *index);
        let selected = if choices.len() > 1 {
            replay_choice(&choices)?.and_then(|index| choices.get(index))
        } else {
            None
        };
        let next = selected.or_else(|| choices.first()).copied();
        // Check both boundaries so skipping an early packet cannot hide a
        // conflict whose result depends on the ordering of distinct packets.
        for entry in [choices.first().copied(), next].into_iter().flatten() {
            let tied = choices
                .iter()
                .copied()
                .filter(|candidate| candidate.2.timestamp == entry.2.timestamp)
                .collect::<Vec<_>>();
            if tied.len() > 1
                && (selected.is_none() || tied.iter().any(|candidate| candidate.1 != entry.1))
            {
                return Err(phase_ambiguity(
                    "Multiple phase transitions at the same time",
                    &tied,
                ));
            }
        }
        if let Some(next) = next
            && next.2.timestamp == current.2.timestamp
        {
            return Err(phase_ambiguity(
                "Phase transition shares the prior entry timestamp",
                &[current, next],
            ));
        }
        // An incoming signal can recur as an ordinary cast. Only outgoing edges
        // establish the next boundary, and replay checks the inferred route.
        let Some(next) = next else { break };
        current = next;
    }
    Ok(route)
}

fn replayed_phase_choice(rows: &[Record], choices: &[PhaseEntry<'_>]) -> Option<usize> {
    for row in rows {
        if row.action == "ambiguous"
            && choices
                .iter()
                .any(|entry| row.source_event_index == Some(entry.1))
        {
            return None;
        }
        if row.action == "phase"
            && let Some(phase) = row
                .context
                .as_ref()
                .and_then(|context| context.owner.phase.as_deref())
            && let Some(index) = choices
                .iter()
                .position(|entry| row.source_event_index == Some(entry.1) && entry.0.phase == phase)
        {
            return Some(index);
        }
    }
    None
}

fn inferred_segments(
    log: &CollectedLog,
    profile: &Profile,
    actors: &BTreeMap<i64, &Actor>,
    origin: i64,
    encounter: i64,
    model: Option<&Timeline>,
) -> Result<Vec<Segment>> {
    let mut roots = profile
        .segments
        .iter()
        .filter(|rule| {
            rule.accepts_entry(None, None) && rule.encounter.is_none_or(|id| id == encounter)
        })
        .filter_map(|rule| {
            find_entries(log, rule, actors, None)
                .next()
                .map(|(index, event)| (rule, index, event))
        })
        .collect::<Vec<_>>();
    roots.sort_by_key(|(_, index, _)| *index);
    let mut sources = BTreeSet::new();
    if roots.is_empty()
        || !roots.iter().all(|(rule, _, _)| {
            profile.continuations.contains(&rule.phase) || sources.insert(&rule.source)
        })
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
    let mut replay_rows: Option<Vec<Record>> = None;
    let mut replay_choice = |choices: &[PhaseEntry<'_>]| {
        let Some(model) = model else {
            return Ok(None);
        };
        if replay_rows.is_none() {
            let end = log.events.last().context("Empty events")?.timestamp - origin;
            let recording = recording(
                log,
                actors,
                origin,
                end,
                &ObservationScope::AllCastsAndStarts,
                |_| true,
            )?;
            replay_rows = Some(crate::timeline::replay(model, &recording)?);
        }
        Ok(replayed_phase_choice(
            replay_rows.as_deref().unwrap_or_default(),
            choices,
        ))
    };
    for (root, limit) in roots.iter().copied().zip(limits) {
        candidates.extend(
            follow_transitions(
                log,
                &profile.segments,
                actors,
                root,
                limit,
                &mut replay_choice,
            )?
            .into_iter()
            .enumerate()
            .map(|(index, entry)| (entry, index != 0)),
        );
    }
    // Guarded transitions use the observed entry; independent boss attempts still require defeat.
    let next_entries = candidates
        .iter()
        .skip(1)
        .map(|((rule, _, event), guarded)| {
            Some(if *guarded || profile.continuations.contains(&rule.phase) {
                Boundary::TransitionAt(event.timestamp)
            } else {
                Boundary::DefeatBefore(event.timestamp)
            })
        })
        .chain(std::iter::once(None));
    candidates
        .iter()
        .zip(next_entries)
        .map(|(((rule, index, event), _), next_entry)| {
            project_segment(
                log,
                rule,
                actors,
                origin,
                profile.inferred,
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
    let source = |id: Option<i64>| id.and_then(|id| actors.get(&id)).map(|a| a.name.as_str());
    let entry = inferred_entry
        .or_else(|| find_entries(log, rule, actors, None).next())
        .ok_or_else(|| exclude("Missing boss entry", json!({"source":rule.source})))?;
    let (entry_event, start) = (entry.0, entry.1.timestamp);
    let transition_only = inferred
        && rule.targets.is_empty()
        && matches!(next_entry, Some(Boundary::TransitionAt(_)));
    let main = main_actors(log, rule, transition_only.then_some(entry))?;
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
    let targets = project_targets(
        log,
        rule,
        start..=end,
        origin,
        &source,
        inferred.then_some(entry_event),
    )?;
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

fn project_targets<'s>(
    log: &CollectedLog,
    rule: &SegmentRule,
    range: std::ops::RangeInclusive<i64>,
    origin: i64,
    source: &impl Fn(Option<i64>) -> Option<&'s str>,
    inferred_entry: Option<usize>,
) -> Result<Vec<Target>> {
    let mut last = BTreeMap::<(bool, &str, &str), (i64, usize)>::new();
    let mut targets = Vec::<Target>::new();
    for (raw_index, event) in log
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == "cast" && range.contains(&e.timestamp))
    {
        // Entry opens the scope and cannot establish an advance warning.
        if inferred_entry == Some(raw_index) {
            continue;
        }
        let id = ability(event)?;
        let Some((id_key, target)) = rule.target_at(&id, event, log, &source)? else {
            continue;
        };
        // Alias packets share only explicitly declared groups, e.g. C3FF and C400 form one Beam wave.
        // Keep explicit groups separate from implicit ability keys, e.g. group="B528" is not ID B528.
        let group = (
            target.group.is_some(),
            target.group.as_deref().unwrap_or(id_key),
            if target.group.is_some() {
                ""
            } else {
                target.source.as_deref().unwrap_or(&rule.source)
            },
        );
        if let Some((_, index)) = last
            .get(&group)
            .filter(|(before, _)| event.timestamp - before <= target.group_window_ms)
        {
            targets
                .get_mut(*index)
                .context("Missing grouped target")?
                .raw_indices
                .push(raw_index);
            continue;
        }
        last.insert(group, (event.timestamp, targets.len()));
        targets.push(target.project(raw_index, event.timestamp - origin, id));
    }
    Ok(targets)
}

fn audit_segment(
    log: &CollectedLog,
    rule: &SegmentRule,
    range: std::ops::RangeInclusive<i64>,
    origin: i64,
) -> Result<super::audit::Audit> {
    let names = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let source_name = |id: Option<i64>| id.and_then(|id| names.get(&id).copied());
    let sources = std::iter::once(rule.source.as_str())
        .chain(rule.other_sources.iter().map(String::as_str))
        .chain(
            rule.targets
                .values()
                .flatten()
                .filter_map(|target| target.source.as_deref()),
        )
        .collect();
    super::audit::collect(
        log,
        range,
        origin,
        &sources,
        |source, id, event| {
            rule.targets.get(id).is_some_and(|targets| {
                targets.iter().any(|target| {
                    source == target.source.as_deref().unwrap_or(&rule.source)
                        && target.in_window(event, log, &source_name, &rule.source)
                })
            })
        },
        &rule.ignored,
    )
}

pub(super) fn project(
    log: &CollectedLog,
    profile: &Profile,
    model: Option<&Timeline>,
) -> Result<Projection> {
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
    let bosses = if profile.inferred
        || profile.segments.iter().any(|rule| {
            rule.entry_from.is_some() && rule.encounter.is_none_or(|id| id == encounter)
        }) {
        inferred_segments(log, profile, &actors, origin, encounter, model)?
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
    let recording = recording(
        log,
        &actors,
        origin,
        end_ms,
        &profile.observations,
        |at_ms| {
            bosses
                .iter()
                .any(|b| at_ms >= b.entry_ms && at_ms <= b.end_ms)
        },
    )?;
    Ok(Projection {
        origin_timestamp_ms: origin,
        recording,
        bosses,
    })
}

fn recording(
    log: &CollectedLog,
    actors: &BTreeMap<i64, &Actor>,
    origin: i64,
    end_ms: i64,
    scope: &ObservationScope,
    in_scope: impl Fn(i64) -> bool,
) -> Result<Recording> {
    let mut observations = Vec::new();
    for (raw_index, event) in log.events.iter().enumerate() {
        let at_ms = event.timestamp - origin;
        if event.kind != "cast"
            && !(event.kind == "begincast"
                && matches!(
                    scope,
                    ObservationScope::ScopedCastsAndStarts | ObservationScope::AllCastsAndStarts
                ))
        {
            continue;
        }
        if !matches!(
            scope,
            ObservationScope::AllCasts | ObservationScope::AllCastsAndStarts
        ) && !in_scope(at_ms)
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
    Ok(Recording {
        version: 1,
        end_ms,
        observations,
    })
}

impl TargetRule {
    fn in_window<'a>(
        &self,
        event: &Event,
        log: &CollectedLog,
        source: &impl Fn(Option<i64>) -> Option<&'a str>,
        default_source: &str,
    ) -> bool {
        let Some(window) = &self.window else {
            return true;
        };
        let anchor = log
            .events
            .iter()
            .filter(|candidate| {
                candidate.kind == "cast"
                    && source(candidate.source_id) == Some(default_source)
                    && ability(candidate).is_ok_and(|id| id == window.anchor)
            })
            .nth(window.occurrence - 1);
        anchor.is_some_and(|anchor| {
            event.timestamp >= anchor.timestamp
                && event.timestamp - anchor.timestamp < window.duration_ms
        })
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            (0..=2000).contains(&self.group_window_ms),
            "group_window_ms must be between 0 and 2000"
        );
        if let Some(window) = &self.window {
            ensure!(
                window.occurrence > 0
                    && (1..=MAX_MS).contains(&window.duration_ms)
                    && !window.anchor.is_empty()
                    && window
                        .anchor
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_lowercase()),
                "Invalid target observation window"
            );
        }
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
            raw_indices: vec![raw_index],
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
            reference: self.reference,
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
        for (id, target) in self
            .targets
            .iter()
            .flat_map(|(id, rules)| rules.iter().map(move |target| (id, target)))
        {
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
                ensure!(
                    target.reference != Reference::ActiveWindow || definition.event.duration_ms > 0,
                    "Active-window binding {event} requires a positive display duration"
                );
            }
            ensure!(
                sets.iter()
                    .all(|previous| previous == &events || previous.is_disjoint(&events)),
                "Overlapping event bindings must declare the same candidate set in {}",
                self.phase
            );
            if let Some(group) = &target.group
                && let Some(previous) = groups.insert(
                    group,
                    (
                        &target.name,
                        events.clone(),
                        target.reference,
                        target.group_window_ms,
                    ),
                )
            {
                ensure!(
                    previous
                        == (
                            &target.name,
                            events.clone(),
                            target.reference,
                            target.group_window_ms
                        ),
                    "Collapsed group {group} must share name and event bindings"
                );
            }
            sets.push(events);
        }
        Ok(())
    }
}

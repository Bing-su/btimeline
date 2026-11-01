use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{Branch, BranchOption, Event, MAX_MS, Phase, Tick, Timeline};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    at_ms: i64,
    #[serde(rename = "type")]
    kind: String,
    fields: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recording {
    version: u32,
    end_ms: i64,
    observations: Vec<Observation>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Record {
    pub(crate) at_ms: i64,
    pub(crate) action: String,
    pub(crate) detail: String,
    pub(crate) run: usize,
    #[serde(skip)]
    event_order: Option<usize>,
}

#[derive(Default)]
struct Clock {
    base_ms: i64,
    not_before: i64,
    processed: BTreeSet<usize>,
}

struct ActivePath {
    anchor: Tick,
    clock: Clock,
}

#[derive(Default)]
struct BranchState {
    opened: bool,
    closed: bool,
    selected: Option<usize>,
    option: Option<ActivePath>,
    merge: Option<ActivePath>,
}

impl BranchState {
    fn paths<'a>(
        &'a self,
        branch: &'a Branch,
    ) -> impl Iterator<Item = (&'a [Event], &'a ActivePath)> {
        let option = self
            .selected
            .and_then(|index| branch.options.get(index))
            .zip(self.option.as_ref())
            .map(|(option, path)| (option.events.as_slice(), path));
        let merge = branch
            .merge
            .as_ref()
            .zip(self.merge.as_ref())
            .map(|(merge, path)| (merge.events.as_slice(), path));
        option.into_iter().chain(merge)
    }

    fn paths_mut<'a>(
        &'a mut self,
        branch: &'a Branch,
    ) -> impl Iterator<Item = (&'a [Event], &'a mut ActivePath)> {
        let option = self
            .selected
            .and_then(|index| branch.options.get(index))
            .zip(self.option.as_mut())
            .map(|(option, path)| (option.events.as_slice(), path));
        let merge = branch
            .merge
            .as_ref()
            .zip(self.merge.as_mut())
            .map(|(merge, path)| (merge.events.as_slice(), path));
        option.into_iter().chain(merge)
    }
}

struct PhaseState<'a> {
    definition: &'a Phase,
    clock: Clock,
    branches: Vec<BranchState>,
    repeat: Option<RepeatState>,
}

#[derive(Default)]
struct RepeatState {
    current: Option<Clock>,
    exited: bool,
    exit: Option<ActivePath>,
    unresolved: bool,
}

enum BranchAction {
    Select(usize, usize),
    Merge(usize),
    Conflict(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SyncOwner<'a> {
    Fight,
    Phase(&'a str),
    Repeat(&'a str),
}

fn matches_any(signals: &[super::Signal], observation: &Observation, time: Tick) -> bool {
    signals
        .iter()
        .any(|signal| signal.matches(&observation.kind, &observation.fields, time))
}

fn active_path(base_ms: i64, at: i64) -> ActivePath {
    let anchor = elapsed(base_ms, at);
    ActivePath {
        anchor,
        clock: Clock {
            base_ms: base_ms + anchor * 100,
            not_before: at,
            ..Clock::default()
        },
    }
}

fn requirement_status(
    option: &BranchOption,
    history: &BTreeMap<String, Option<String>>,
) -> (bool, bool) {
    let mut missing = false;
    let mut mismatch = false;
    for (branch, required) in &option.requires {
        match history.get(branch) {
            Some(Some(selected)) if selected == required => {}
            Some(Some(_)) => mismatch = true,
            _ => missing = true,
        }
    }
    (missing, mismatch)
}

fn option_actions(
    branch: &Branch,
    state: &BranchState,
    index: usize,
    time: Tick,
    observation: &Observation,
    history: &BTreeMap<String, Option<String>>,
) -> (Vec<BranchAction>, Vec<String>) {
    let mut actions = Vec::new();
    let mut unresolved = Vec::new();
    for (option_index, option) in branch.options.iter().enumerate() {
        if !matches_any(&option.whens, observation, time) {
            continue;
        }
        let (missing, mismatch) = requirement_status(option, history);
        if mismatch {
            continue;
        }
        if missing {
            unresolved.push(branch.id.clone());
            continue;
        }
        match state.selected {
            None => actions.push(BranchAction::Select(index, option_index)),
            Some(selected) if selected != option_index => {
                actions.push(BranchAction::Conflict(index))
            }
            _ => {}
        }
    }
    (actions, unresolved)
}

impl<'a> PhaseState<'a> {
    fn new(definition: &'a Phase, at: i64) -> Self {
        Self {
            definition,
            clock: Clock {
                base_ms: at,
                ..Clock::default()
            },
            branches: definition
                .branches
                .iter()
                .map(|_| BranchState::default())
                .collect(),
            repeat: definition.loops.first().map(|_| RepeatState::default()),
        }
    }
}

// Round relative observations without changing raw input order, e.g. 1049ms -> 1.0s.
fn elapsed(base_ms: i64, at_ms: i64) -> Tick {
    (at_ms - base_ms + 50).div_euclid(100)
}

fn record(out: &mut Vec<Record>, at_ms: i64, action: &str, detail: impl Into<String>, run: usize) {
    out.push(Record {
        at_ms,
        action: action.into(),
        detail: detail.into(),
        run,
        event_order: None,
    });
}

fn emit(
    events: &[Event],
    clock: &mut Clock,
    until: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    let mut pending: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            let at = (clock.base_ms + e.at * 100).max(clock.not_before);
            !clock.processed.contains(i) && (at < until || (inclusive && at == until))
        })
        .collect();
    pending.sort_by_key(|(i, event)| (event.at, *i));
    for (i, event) in pending {
        clock.processed.insert(i);
        record(
            out,
            (clock.base_ms + event.at * 100).max(clock.not_before),
            "event",
            format!(
                "{}: {} (visible={}, duration={:.1}s)",
                event.id,
                event.name,
                event.visible,
                event.duration as f64 / 10.0
            ),
            run,
        );
        if let Some(record) = out.last_mut() {
            record.event_order = Some(event.order);
        }
    }
}

fn correct(
    events: &[Event],
    clock: &mut Clock,
    at_ms: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
) {
    let previous = elapsed(clock.base_ms, at_ms);
    if target > previous {
        for (i, event) in events.iter().enumerate() {
            if event.at < target && !clock.processed.contains(&i) {
                clock.processed.insert(i);
                record(out, at_ms, "skipped", event.id.clone(), run);
            }
        }
    }
    clock.base_ms = at_ms - target * 100;
}

// Validate raw order before any state transition, e.g. never sort malformed recordings silently.
fn validate_recording(input: &Recording) -> Result<()> {
    ensure!(
        input.version == 1,
        "Unsupported recording version; expected 1"
    );
    ensure!(
        (0..=MAX_MS).contains(&input.end_ms),
        "Recording end_ms out of range"
    );
    let mut previous = 0;
    for observation in &input.observations {
        ensure!(
            (previous..=input.end_ms).contains(&observation.at_ms),
            "Observations must be in original time order within 0..end_ms"
        );
        ensure!(
            !observation.kind.is_empty(),
            "Observation type must not be empty"
        );
        previous = observation.at_ms;
    }
    Ok(())
}

// Compute candidates before applying a correction so overlapping windows see the same clock.
fn sync_targets(events: &[Event], clock: &Clock, observation: &Observation) -> BTreeSet<Tick> {
    let time = elapsed(clock.base_ms, observation.at_ms);
    events
        .iter()
        .flat_map(|e| &e.syncs)
        .filter(|s| s.matches(&observation.kind, &observation.fields, time))
        .filter_map(|s| s.at)
        .collect()
}

// Keep censored future events distinct from skipped events caused by a forward correction.
fn unobserved(events: &[Event], clock: &Clock, end_ms: i64, run: usize, out: &mut Vec<Record>) {
    for (_, event) in events
        .iter()
        .enumerate()
        .filter(|(i, _)| !clock.processed.contains(i))
    {
        record(out, end_ms, "unobserved", event.id.clone(), run);
    }
}

fn advance_paths(
    phase: &mut PhaseState<'_>,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    for (branch, state) in phase.definition.branches.iter().zip(&mut phase.branches) {
        for (events, path) in state.paths_mut(branch) {
            emit(events, &mut path.clock, at, inclusive, out, run);
        }
    }
}

fn branch_sync_targets<'a>(
    phase: &PhaseState<'a>,
    observation: &Observation,
) -> Vec<(SyncOwner<'a>, Tick)> {
    phase
        .definition
        .branches
        .iter()
        .zip(&phase.branches)
        .flat_map(|(branch, state)| state.paths(branch))
        .flat_map(|(events, path)| {
            sync_targets(events, &path.clock, observation)
                .into_iter()
                .map(move |target| {
                    (
                        SyncOwner::Phase(phase.definition.id.as_str()),
                        path.anchor + target,
                    )
                })
        })
        .collect()
}

fn repeat_sync_targets<'a>(
    phase: &PhaseState<'a>,
    observation: &Observation,
) -> Vec<(SyncOwner<'a>, Tick)> {
    let (Some(repeat), Some(state)) = (phase.definition.loops.first(), phase.repeat.as_ref())
    else {
        return Vec::new();
    };
    let mut targets = Vec::new();
    if let Some(path) = &state.exit {
        targets.extend(
            sync_targets(&repeat.on_exit, &path.clock, observation)
                .into_iter()
                .map(|t| {
                    (
                        SyncOwner::Phase(phase.definition.id.as_str()),
                        path.anchor + t,
                    )
                }),
        );
    }
    let at = observation.at_ms;
    let boundary = !state.exited
        && state.current.as_ref().map_or_else(
            || phase.clock.base_ms + repeat.start * 100 == at,
            |clock| clock.base_ms + repeat.every * 100 == at,
        );
    let boundary_clock = Clock {
        base_ms: at,
        ..Clock::default()
    };
    let clock = if boundary {
        Some(&boundary_clock)
    } else {
        state.current.as_ref()
    };
    if let Some(clock) = clock {
        targets.extend(
            sync_targets(&repeat.events, clock, observation)
                .into_iter()
                .map(|t| (SyncOwner::Repeat(repeat.id.as_str()), t)),
        );
    }
    targets
}

// At a boundary the observation can synchronize the new iteration before its zero-time event.
fn prepare_repeat_boundary(phase: &mut PhaseState<'_>, at: i64) {
    let (Some(definition), Some(state)) = (phase.definition.loops.first(), phase.repeat.as_mut())
    else {
        return;
    };
    if state.exited {
        return;
    }
    let start = phase.clock.base_ms + definition.start * 100;
    if state.current.is_none() && start == at {
        state.current = Some(Clock {
            base_ms: at,
            ..Clock::default()
        });
        return;
    }
    let Some(clock) = &mut state.current else {
        return;
    };
    if clock.base_ms + definition.every * 100 == at {
        clock.base_ms = at;
        clock.processed.clear();
    }
}

fn correct_paths(
    phase: &mut PhaseState<'_>,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
) {
    for (branch, state) in phase.definition.branches.iter().zip(&mut phase.branches) {
        for (events, path) in state.paths_mut(branch) {
            correct(events, &mut path.clock, at, target - path.anchor, out, run);
        }
    }
}

// A forward phase correction can enter a loop after skipping whole iterations.
fn enter_repeat_after_jump(
    phase: &mut PhaseState<'_>,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
) -> Result<()> {
    let (Some(repeat), Some(state)) = (phase.definition.loops.first(), phase.repeat.as_mut())
    else {
        return Ok(());
    };
    if state.exited || state.current.is_some() || target <= repeat.start {
        return Ok(());
    }
    let offset = target - repeat.start;
    let completed = offset / repeat.every;
    let position = offset % repeat.every;
    let event_count = i64::try_from(repeat.events.len()).unwrap_or(i64::MAX);
    let partial_count = i64::try_from(
        repeat
            .events
            .iter()
            .filter(|event| event.at < position)
            .count(),
    )
    .unwrap_or(i64::MAX);
    ensure!(
        completed <= 1_000_000
            && completed
                .saturating_mul(event_count)
                .saturating_add(partial_count)
                <= 1_000_000,
        "Phase sync would skip more than one million loop iterations or events in {}",
        repeat.id
    );

    let mut events: Vec<_> = repeat.events.iter().enumerate().collect();
    events.sort_by_key(|(_, event)| (event.at, event.order));
    for _ in 0..completed {
        for (_, event) in &events {
            record(out, at, "skipped", event.id.clone(), run);
        }
    }
    let mut clock = Clock {
        base_ms: at - position * 100,
        not_before: at,
        ..Clock::default()
    };
    for (index, event) in events {
        if event.at < position {
            clock.processed.insert(index);
            record(out, at, "skipped", event.id.clone(), run);
        }
    }
    state.current = Some(clock);
    Ok(())
}

fn correct_phase(
    phase: &mut PhaseState<'_>,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
) -> Result<()> {
    enter_repeat_after_jump(phase, at, target, out, run)?;
    correct(
        &phase.definition.events,
        &mut phase.clock,
        at,
        target,
        out,
        run,
    );
    correct_paths(phase, at, target, out, run);
    if let (Some(repeat), Some(path)) = (
        phase.definition.loops.first(),
        phase.repeat.as_mut().and_then(|state| state.exit.as_mut()),
    ) {
        correct(
            &repeat.on_exit,
            &mut path.clock,
            at,
            target - path.anchor,
            out,
            run,
        );
    }
    Ok(())
}

fn unobserved_paths(phase: &PhaseState<'_>, at: i64, run: usize, out: &mut Vec<Record>) {
    for (branch, state) in phase.definition.branches.iter().zip(&phase.branches) {
        for (events, path) in state.paths(branch) {
            unobserved(events, &path.clock, at, run, out);
        }
    }
}

fn due(at: i64, limit: i64, inclusive: bool) -> bool {
    at < limit || (inclusive && at == limit)
}

// Close each completed iteration before emitting the next iteration's events.
fn advance_repeat(
    phase: &mut PhaseState<'_>,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    let (Some(definition), Some(state)) = (phase.definition.loops.first(), phase.repeat.as_mut())
    else {
        return;
    };
    if let Some(path) = &mut state.exit {
        emit(
            &definition.on_exit,
            &mut path.clock,
            at,
            inclusive,
            out,
            run,
        );
    }
    if state.exited {
        return;
    }
    let start = phase.clock.base_ms + definition.start * 100;
    if state.current.is_none() && due(start, at, inclusive) {
        state.current = Some(Clock {
            base_ms: start,
            ..Clock::default()
        });
    }
    let Some(clock) = &mut state.current else {
        return;
    };
    let period = definition.every * 100;
    while due(clock.base_ms + period, at, inclusive) {
        emit(
            &definition.events,
            clock,
            clock.base_ms + period,
            false,
            out,
            run,
        );
        clock.base_ms += period;
        clock.processed.clear();
    }
    emit(&definition.events, clock, at, inclusive, out, run);
}

// Report a missed finite exit only after all alternative windows have ended.
fn repeat_unresolved(
    phase: &mut PhaseState<'_>,
    at: i64,
    corrected: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    let (Some(definition), Some(state)) = (phase.definition.loops.first(), phase.repeat.as_mut())
    else {
        return;
    };
    if state.exited || state.unresolved || definition.untils.is_empty() {
        return;
    }
    let end = definition
        .untils
        .iter()
        .map(|signal| signal.at.zip(signal.after).map(|(at, after)| at + after))
        .collect::<Option<Vec<_>>>();
    let Some(end) = end.and_then(|ends| ends.into_iter().max()) else {
        return;
    };
    if elapsed(phase.clock.base_ms, at) <= end {
        return;
    }
    state.unresolved = true;
    let ended_at = (phase.clock.base_ms + (end + 1) * 100 - 50).max(0);
    record(
        out,
        if corrected { at } else { ended_at },
        "unresolved",
        definition.id.clone(),
        run,
    );
}

fn unobserved_repeat(phase: &PhaseState<'_>, at: i64, out: &mut Vec<Record>, run: usize) {
    let (Some(definition), Some(state)) = (phase.definition.loops.first(), &phase.repeat) else {
        return;
    };
    if let Some(clock) = &state.current {
        unobserved(&definition.events, clock, at, run, out);
    }
    if let Some(path) = &state.exit {
        unobserved(&definition.on_exit, &path.clock, at, run, out);
    }
}

struct Replay<'a> {
    timeline: &'a Timeline,
    fight: Option<Clock>,
    phase: Option<PhaseState<'a>>,
    history: BTreeMap<String, Option<String>>,
    run: usize,
    out: Vec<Record>,
}

impl<'a> Replay<'a> {
    // Advance only currently active schedules, preserving cancellation at transition timestamps.
    fn advance(&mut self, at: i64, inclusive: bool) {
        if let Some(clock) = self.fight.as_mut() {
            emit(
                &self.timeline.events,
                clock,
                at,
                inclusive,
                &mut self.out,
                self.run,
            );
        }
        if let Some(phase) = self.phase.as_mut() {
            emit(
                &phase.definition.events,
                &mut phase.clock,
                at,
                inclusive,
                &mut self.out,
                self.run,
            );
            advance_paths(phase, at, inclusive, &mut self.out, self.run);
            advance_repeat(phase, at, inclusive, &mut self.out, self.run);
        }
    }

    fn repeat_observation(&mut self, observation: &Observation) -> bool {
        let Some(phase) = &mut self.phase else {
            return false;
        };
        let (Some(definition), Some(state)) =
            (phase.definition.loops.first(), phase.repeat.as_mut())
        else {
            return false;
        };
        let time = elapsed(phase.clock.base_ms, observation.at_ms);
        if state.exited || !matches_any(&definition.untils, observation, time) {
            return false;
        }
        state.exited = true;
        state.current = None;
        state.exit = Some(active_path(phase.clock.base_ms, observation.at_ms));
        record(
            &mut self.out,
            observation.at_ms,
            "until",
            definition.id.clone(),
            self.run,
        );
        true
    }

    // Window comparisons use rounded phase time, including at the observation boundary.
    fn advance_branches(&mut self, at: i64, corrected: bool) {
        let Some(phase) = &mut self.phase else {
            return;
        };
        let time = elapsed(phase.clock.base_ms, at);
        for (branch, state) in phase.definition.branches.iter().zip(&mut phase.branches) {
            if !state.opened && time >= branch.open {
                state.opened = true;
                self.history.insert(branch.id.clone(), None);
            }
            if !state.opened || state.closed || time < branch.close {
                continue;
            }
            state.closed = true;
            if state.selected.is_some() {
                continue;
            }
            let close_at = (phase.clock.base_ms + branch.close * 100 - 50).max(0);
            let record_at = if corrected { at } else { close_at };
            record(
                &mut self.out,
                record_at,
                "unresolved",
                branch.id.clone(),
                self.run,
            );
        }
    }

    fn branch_actions(&self, observation: &Observation) -> (Vec<BranchAction>, Vec<String>) {
        let Some(phase) = &self.phase else {
            return (Vec::new(), Vec::new());
        };
        let time = elapsed(phase.clock.base_ms, observation.at_ms);
        let mut actions = Vec::new();
        let mut unresolved = Vec::new();
        for (index, (branch, state)) in phase
            .definition
            .branches
            .iter()
            .zip(&phase.branches)
            .enumerate()
        {
            if !state.opened || state.merge.is_some() {
                continue;
            }
            if branch
                .merge
                .as_ref()
                .is_some_and(|merge| matches_any(&merge.whens, observation, time))
            {
                actions.push(BranchAction::Merge(index));
            }
            if state.closed {
                continue;
            }
            let (candidates, missing) =
                option_actions(branch, state, index, time, observation, &self.history);
            actions.extend(candidates);
            unresolved.extend(missing);
        }
        (actions, unresolved)
    }

    fn branch_observation(&mut self, observation: &Observation) -> bool {
        let (actions, unresolved) = self.branch_actions(observation);
        for id in &unresolved {
            record(
                &mut self.out,
                observation.at_ms,
                "unresolved",
                format!("{id}: missing prior choice"),
                self.run,
            );
        }
        if actions.len() > 1 {
            record(
                &mut self.out,
                observation.at_ms,
                "ambiguous",
                "Multiple branch actions",
                self.run,
            );
            return true;
        }
        let Some(action) = actions.first() else {
            return !unresolved.is_empty();
        };
        let phase = self.phase.as_mut().expect("branch actions require a phase");
        let (index, option) = match action {
            BranchAction::Select(index, option) => (*index, Some(*option)),
            BranchAction::Merge(index) | BranchAction::Conflict(index) => (*index, None),
        };
        let (Some(branch), Some(state)) = (
            phase.definition.branches.get(index),
            phase.branches.get_mut(index),
        ) else {
            return true;
        };
        match action {
            BranchAction::Select(_, _) => {
                let option_index = option.expect("selection has an option");
                let Some(choice) = branch.options.get(option_index) else {
                    return true;
                };
                state.selected = Some(option_index);
                state.option = Some(active_path(phase.clock.base_ms, observation.at_ms));
                self.history
                    .insert(branch.id.clone(), Some(choice.id.clone()));
                record(
                    &mut self.out,
                    observation.at_ms,
                    "option",
                    format!("{}/{}", branch.id, choice.id),
                    self.run,
                );
            }
            BranchAction::Merge(_) => {
                if state.selected.is_none() && !state.closed {
                    record(
                        &mut self.out,
                        observation.at_ms,
                        "unresolved",
                        branch.id.clone(),
                        self.run,
                    );
                }
                state.closed = true;
                state.option = None;
                state.merge = Some(active_path(phase.clock.base_ms, observation.at_ms));
                record(
                    &mut self.out,
                    observation.at_ms,
                    "merge",
                    branch.id.clone(),
                    self.run,
                );
            }
            BranchAction::Conflict(_) => record(
                &mut self.out,
                observation.at_ms,
                "ambiguous",
                format!("Conflicting option for {}", branch.id),
                self.run,
            ),
        }
        true
    }

    fn matches_lifecycle(&self, role: &str, observation: &Observation) -> bool {
        self.timeline.lifecycle.get(role).is_some_and(|signals| {
            signals
                .iter()
                .any(|s| s.matches(&observation.kind, &observation.fields, 0))
        })
    }

    // Capture the old phase and clock before boss-complete can close them.
    fn enter_candidates(&self, observation: &Observation) -> Vec<&'a Phase> {
        if self.fight.is_none() && !self.matches_lifecycle("start", observation) {
            return Vec::new();
        }
        let old_phase = self.phase.as_ref().map(|p| p.definition.id.as_str());
        let time = self
            .phase
            .as_ref()
            .map_or(0, |p| elapsed(p.clock.base_ms, observation.at_ms));
        self.timeline
            .phases
            .iter()
            .filter(|p| Some(p.id.as_str()) != old_phase)
            .filter(|p| {
                p.enters.iter().any(|s| {
                    let from_ok = s
                        .from
                        .as_ref()
                        .is_none_or(|from| old_phase == Some(from.as_str()));
                    from_ok && s.matches(&observation.kind, &observation.fields, time)
                })
            })
            .collect()
    }

    fn lifecycle(&self, observation: &Observation) -> Option<&'static str> {
        ["zone-exit", "wipe", "finish", "boss-complete", "start"]
            .into_iter()
            .find(|role| {
                (*role != "start" || self.fight.is_none())
                    && self.matches_lifecycle(role, observation)
            })
    }

    // Reset only lifecycle-owned state; boss completion preserves the fight clock and run.
    fn apply_lifecycle(&mut self, role: &str, at: i64) {
        if role == "wipe" {
            self.record_unobserved(at);
        }
        match role {
            "start" => {
                self.history.clear();
                self.run += 1;
                self.fight = Some(Clock {
                    base_ms: at,
                    ..Clock::default()
                });
            }
            "boss-complete" => self.phase = None,
            _ => {
                self.fight = None;
                self.phase = None;
                self.history.clear();
            }
        }
        record(&mut self.out, at, role, "", self.run);
    }

    fn enter_phase(&mut self, candidates: &[&'a Phase], at: i64) {
        if let [phase] = candidates {
            self.phase = Some(PhaseState::new(phase, at));
            record(&mut self.out, at, "phase", phase.id.clone(), self.run);
        } else {
            record(
                &mut self.out,
                at,
                "ambiguous",
                "Multiple phase destinations",
                self.run,
            );
        }
    }

    fn synchronize(&mut self, observation: &Observation) -> Result<()> {
        let at = observation.at_ms;
        let mut targets = BTreeSet::new();
        if let Some(clock) = self.fight.as_ref() {
            targets.extend(
                sync_targets(&self.timeline.events, clock, observation)
                    .into_iter()
                    .map(|t| (SyncOwner::Fight, t)),
            );
        }
        if let Some(phase) = self.phase.as_ref() {
            targets.extend(
                sync_targets(&phase.definition.events, &phase.clock, observation)
                    .into_iter()
                    .map(|t| (SyncOwner::Phase(phase.definition.id.as_str()), t)),
            );
            targets.extend(branch_sync_targets(phase, observation));
            targets.extend(repeat_sync_targets(phase, observation));
        }
        if targets.len() > 1 {
            record(
                &mut self.out,
                at,
                "ambiguous",
                "Multiple sync destinations; clocks unchanged",
                self.run,
            );
            return Ok(());
        }
        let Some((owner, target)) = targets.first() else {
            return Ok(());
        };
        match (owner, self.phase.as_mut(), self.fight.as_mut()) {
            (SyncOwner::Phase(_), Some(phase), _) => {
                correct_phase(phase, at, *target, &mut self.out, self.run)?
            }
            (SyncOwner::Fight, _, Some(clock)) => correct(
                &self.timeline.events,
                clock,
                at,
                *target,
                &mut self.out,
                self.run,
            ),
            (SyncOwner::Repeat(_), Some(phase), _) => {
                prepare_repeat_boundary(phase, at);
                if let (Some(repeat), Some(clock)) = (
                    phase.definition.loops.first(),
                    phase
                        .repeat
                        .as_mut()
                        .and_then(|state| state.current.as_mut()),
                ) {
                    correct(&repeat.events, clock, at, *target, &mut self.out, self.run);
                }
            }
            _ => {}
        }
        record(
            &mut self.out,
            at,
            "sync",
            format!(
                "{} -> {:.1}s",
                match owner {
                    SyncOwner::Fight => "fight",
                    SyncOwner::Phase(id) | SyncOwner::Repeat(id) => id,
                },
                *target as f64 / 10.0
            ),
            self.run,
        );
        Ok(())
    }

    // Apply observations in source order before firing events at the same raw timestamp.
    fn observe(&mut self, observation: &Observation) -> Result<()> {
        let at = observation.at_ms;
        self.advance(at, false);
        self.advance_branches(at, false);
        if let Some(phase) = &mut self.phase {
            repeat_unresolved(phase, at, false, &mut self.out, self.run);
        }
        let candidates = self.enter_candidates(observation);
        let lifecycle = self.lifecycle(observation);
        if let Some(role) = lifecycle {
            self.apply_lifecycle(role, at);
        }
        let terminal = matches!(lifecycle, Some("zone-exit" | "wipe" | "finish"));
        let enter = !terminal && self.fight.is_some() && !candidates.is_empty();
        if enter {
            self.enter_phase(&candidates, at);
        }
        if lifecycle.is_none()
            && !enter
            && self.fight.is_some()
            && !self.repeat_observation(observation)
            && !self.branch_observation(observation)
        {
            self.synchronize(observation)?;
            self.advance_branches(at, true);
            if let Some(phase) = &mut self.phase {
                repeat_unresolved(phase, at, true, &mut self.out, self.run);
            }
        }
        Ok(())
    }

    // A wipe censors the current attempt just like a recording ending before its future events.
    fn record_unobserved(&mut self, end_ms: i64) {
        if let Some(clock) = self.fight.as_ref() {
            unobserved(
                &self.timeline.events,
                clock,
                end_ms,
                self.run,
                &mut self.out,
            );
        }
        if let Some(phase) = self.phase.as_ref() {
            unobserved(
                &phase.definition.events,
                &phase.clock,
                end_ms,
                self.run,
                &mut self.out,
            );
            unobserved_paths(phase, end_ms, self.run, &mut self.out);
            unobserved_repeat(phase, end_ms, &mut self.out, self.run);
        }
    }

    fn finish(mut self, end_ms: i64) -> Vec<Record> {
        self.advance(end_ms, true);
        self.advance_branches(end_ms, false);
        if let Some(phase) = &mut self.phase {
            repeat_unresolved(phase, end_ms, false, &mut self.out, self.run);
        }
        self.record_unobserved(end_ms);
        // Controls precede same-time events; authored events keep document order across paths.
        self.out.sort_by_key(|record| {
            (
                record.at_ms,
                record.event_order.is_some(),
                record.event_order,
            )
        });
        self.out
    }
}

fn validate_repeat_support(timeline: &Timeline, input: &Recording) -> Result<()> {
    for phase in &timeline.phases {
        if let Some(repeat) = phase
            .branches
            .iter()
            .flat_map(|branch| &branch.options)
            .flat_map(|option| &option.loops)
            .next()
        {
            anyhow::bail!(
                "Replay does not yet support loop {} inside an option",
                repeat.id
            );
        }
        ensure!(
            phase.loops.len() <= 1,
            "Replay does not yet support multiple loops in one phase"
        );
        let Some(repeat) = phase.loops.first() else {
            continue;
        };
        ensure!(
            phase.branches.is_empty() && repeat.branches.is_empty(),
            "Replay does not yet support branches and loops in the same phase"
        );
        ensure!(
            input.end_ms / (repeat.every * 100) <= 1_000_000,
            "Recording spans more than one million possible loop iterations; shorten the recording"
        );
    }
    Ok(())
}

pub(crate) fn replay(timeline: &Timeline, input: &Recording) -> Result<Vec<Record>> {
    validate_recording(input)?;
    validate_repeat_support(timeline, input)?;
    let mut state = Replay {
        timeline,
        fight: None,
        phase: None,
        history: BTreeMap::new(),
        run: 0,
        out: Vec::new(),
    };
    let mut observations = input.observations.iter().peekable();
    while let Some(observation) = observations.next() {
        state.observe(observation)?;
        if observations
            .peek()
            .is_none_or(|next| next.at_ms != observation.at_ms)
        {
            state.advance(observation.at_ms, true);
        }
    }
    Ok(state.finish(input.end_ms))
}

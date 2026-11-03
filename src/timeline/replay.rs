use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    Branch,
    BranchOption,
    Event,
    EventData,
    ExecutionContext,
    Loop,
    MAX_MS,
    Owner,
    Phase,
    Tick,
    Timeline,
};

pub(super) mod forecast;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    pub(crate) at_ms: i64,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) fields: BTreeMap<String, String>,
    #[serde(skip)]
    pub(crate) source_event_index: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recording {
    pub(crate) version: u32,
    pub(crate) end_ms: i64,
    pub(crate) observations: Vec<Observation>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplaySummary {
    status: &'static str,
    observations: usize,
    started_runs: usize,
}

impl Recording {
    // Report whether replay ran, e.g. an ACT start cannot be inferred from FFLogs casts.
    pub(crate) fn summary(&self, records: &[Record]) -> ReplaySummary {
        let started_runs = records
            .iter()
            .filter(|record| record.action == "start")
            .count();
        let status = if self.observations.is_empty() {
            "no_observations"
        } else if started_runs == 0 {
            "not_started"
        } else {
            "replayed"
        };
        ReplaySummary {
            status,
            observations: self.observations.len(),
            started_runs,
        }
    }
}

impl std::fmt::Display for ReplaySummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} observations, {} started runs)",
            self.status, self.observations, self.started_runs
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Record {
    pub(crate) at_ms: i64,
    pub(crate) action: String,
    pub(crate) detail: String,
    pub(crate) run: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_event_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) event: Option<EventData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) context: Option<ExecutionContext>,
    #[serde(skip)]
    event_order: Option<usize>,
}

#[derive(Clone, Default)]
struct Clock {
    base_ms: i64,
    not_before: i64,
    zero_at: Option<i64>,
    processed: BTreeSet<usize>,
    phase_instance: usize,
    iteration: Option<u64>,
}

#[derive(Clone)]
struct ActivePath {
    anchor: Tick,
    clock: Clock,
}

#[derive(Clone, Default)]
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
    loops: Vec<LoopRef<'a>>,
    repeats: Vec<RepeatState>,
}

struct LoopRef<'a> {
    definition: &'a Loop,
    owner: Option<(usize, usize)>,
}

#[derive(Clone, Default)]
struct RepeatState {
    phase_instance: usize,
    anchor: Option<Tick>,
    not_before: i64,
    zero_start: Option<i64>,
    current: Option<Clock>,
    exited: bool,
    exit: Option<ActivePath>,
    unresolved: bool,
    blocked: bool,
    branches: Vec<BranchState>,
}

enum BranchAction {
    Select(usize, usize),
    Merge(usize),
    Conflict(usize),
    RepeatSelect(usize, usize, usize),
    RepeatMerge(usize, usize),
    RepeatConflict(usize, usize),
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

fn active_path(parent: &Clock, at: i64) -> ActivePath {
    let base_ms = parent.base_ms;
    let anchor = elapsed(base_ms, at);
    ActivePath {
        anchor,
        clock: Clock {
            base_ms: base_ms + anchor * 100,
            not_before: at,
            zero_at: Some(at),
            phase_instance: parent.phase_instance,
            iteration: parent.iteration,
            ..Clock::default()
        },
    }
}

fn repeat_start(base_ms: i64, repeat: &Loop, state: &RepeatState) -> Option<i64> {
    state.anchor.map(|anchor| {
        let nominal = base_ms + (anchor + repeat.start) * 100;
        if repeat.start == 0 {
            state.zero_start.unwrap_or(nominal)
        } else {
            nominal
        }
        .max(state.not_before)
    })
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
    fn new(definition: &'a Phase, at: i64, phase_instance: usize) -> Self {
        let mut loops: Vec<_> = definition
            .loops
            .iter()
            .map(|repeat| LoopRef {
                definition: repeat,
                owner: None,
            })
            .collect();
        for (branch_index, branch) in definition.branches.iter().enumerate() {
            for (option_index, option) in branch.options.iter().enumerate() {
                loops.extend(option.loops.iter().map(|repeat| LoopRef {
                    definition: repeat,
                    owner: Some((branch_index, option_index)),
                }));
            }
        }
        loops.sort_by_key(|repeat| repeat.definition.order);
        let repeats = loops
            .iter()
            .map(|repeat| RepeatState {
                phase_instance,
                anchor: repeat.owner.is_none().then_some(0),
                not_before: at,
                ..RepeatState::default()
            })
            .collect();
        Self {
            definition,
            clock: Clock {
                base_ms: at,
                phase_instance,
                ..Clock::default()
            },
            branches: definition
                .branches
                .iter()
                .map(|_| BranchState::default())
                .collect(),
            loops,
            repeats,
        }
    }

    fn activate_option_loops(&mut self, branch: usize, option: usize, anchor: Tick, at: i64) {
        for (entry, state) in self.loops.iter().zip(&mut self.repeats) {
            if entry.owner == Some((branch, option)) {
                *state = RepeatState {
                    phase_instance: self.clock.phase_instance,
                    anchor: Some(anchor),
                    not_before: at,
                    zero_start: Some(at),
                    ..RepeatState::default()
                };
            }
        }
    }

    fn cancel_option_loops(&mut self, branch: usize) {
        for (entry, state) in self.loops.iter().zip(&mut self.repeats) {
            if entry.owner.is_some_and(|(owner, _)| owner == branch) {
                *state = RepeatState::default();
            }
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
        source_event_index: None,
        event: None,
        context: None,
        event_order: None,
    });
}

fn execution_context(clock: &Clock, owner: Owner) -> ExecutionContext {
    ExecutionContext {
        owner,
        phase_instance: clock.phase_instance,
        iteration: clock.iteration,
    }
}

fn branch_owner(phase: &str, branch: &str, option: Option<&str>) -> Owner {
    Owner {
        phase: Some(phase.into()),
        branch: Some(branch.into()),
        option: option.map(String::from),
        ..Owner::default()
    }
}

fn record_control(
    out: &mut Vec<Record>,
    at: i64,
    action: &str,
    detail: String,
    run: usize,
    clock: &Clock,
    owner: Owner,
) {
    record(out, at, action, detail, run);
    if let Some(row) = out.last_mut() {
        row.context = Some(execution_context(clock, owner));
    }
}

fn event_time(clock: &Clock, event: &Event) -> i64 {
    let nominal = clock.base_ms + event.at * 100;
    let at = if event.at == 0 {
        clock.zero_at.unwrap_or(nominal)
    } else {
        nominal
    };
    at.max(clock.not_before)
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
            let at = event_time(clock, e);
            !clock.processed.contains(i) && (at < until || (inclusive && at == until))
        })
        .collect();
    pending.sort_by_key(|(i, event)| (event.at, *i));
    for (i, event) in pending {
        clock.processed.insert(i);
        record(
            out,
            event_time(clock, event),
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
            record.event = Some(EventData {
                id: event.id.clone(),
                name: event.name.clone(),
                visible: event.visible,
                duration_ms: event.duration * 100,
            });
            record.context = Some(execution_context(clock, Owner::default()));
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
    clock.zero_at = None;
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
    let at = observation.at_ms;
    let running = phase.repeats.iter().any(|state| state.current.is_some());
    let eligible_start = (!running)
        .then(|| {
            phase
                .loops
                .iter()
                .zip(&phase.repeats)
                .position(|(entry, state)| {
                    !state.exited
                        && !state.blocked
                        && state.current.is_none()
                        && repeat_start(phase.clock.base_ms, entry.definition, state) == Some(at)
                })
        })
        .flatten();
    phase
        .loops
        .iter()
        .zip(&phase.repeats)
        .enumerate()
        .flat_map(|(index, (entry, state))| {
            repeat_sync_targets_one(
                phase,
                entry.definition,
                state,
                observation,
                eligible_start == Some(index),
            )
        })
        .collect()
}

fn repeat_sync_targets_one<'a>(
    phase: &PhaseState<'a>,
    repeat: &'a Loop,
    state: &RepeatState,
    observation: &Observation,
    eligible_start: bool,
) -> Vec<(SyncOwner<'a>, Tick)> {
    if state.anchor.is_none() {
        return Vec::new();
    }
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
        && !state.blocked
        && state.current.as_ref().map_or_else(
            || eligible_start,
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
    if !boundary && state.current.is_some() {
        for (branch, branch_state) in repeat.branches.iter().zip(&state.branches) {
            for (events, path) in branch_state.paths(branch) {
                targets.extend(
                    sync_targets(events, &path.clock, observation)
                        .into_iter()
                        .map(|t| (SyncOwner::Repeat(repeat.id.as_str()), path.anchor + t)),
                );
            }
        }
    }
    targets
}

// At a boundary the observation can synchronize the new iteration before its zero-time event.
fn prepare_repeat_boundary(phase: &mut PhaseState<'_>, id: &str, at: i64) {
    let Some(index) = phase
        .loops
        .iter()
        .position(|repeat| repeat.definition.id == id)
    else {
        return;
    };
    let (Some(definition), Some(state)) = (
        phase.loops.get(index).map(|entry| entry.definition),
        phase.repeats.get_mut(index),
    ) else {
        return;
    };
    if state.exited || state.blocked {
        return;
    }
    if state.current.is_none() && repeat_start(phase.clock.base_ms, definition, state) == Some(at) {
        state.current = Some(Clock {
            base_ms: at,
            phase_instance: state.phase_instance,
            iteration: Some(1),
            ..Clock::default()
        });
        return;
    }
    let Some(clock) = &mut state.current else {
        return;
    };
    if clock.base_ms + definition.every * 100 == at {
        clock.base_ms = at;
        clock.iteration = clock.iteration.map(|iteration| iteration + 1);
        clock.zero_at = None;
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
    history: &mut BTreeMap<String, Option<String>>,
) -> Result<()> {
    let mut pending: Vec<_> = phase
        .loops
        .iter()
        .zip(&phase.repeats)
        .enumerate()
        .filter(|(_, (entry, state))| {
            !state.exited
                && !state.blocked
                && state.current.is_none()
                && state
                    .anchor
                    .is_some_and(|anchor| target > anchor + entry.definition.start)
        })
        .map(|(index, (entry, state))| (state.anchor.unwrap_or(0) + entry.definition.start, index))
        .collect();
    pending.sort_unstable();
    let mut running = phase.repeats.iter().any(|state| state.current.is_some());
    for (_, index) in pending {
        let (Some(repeat), Some(state)) = (
            phase.loops.get(index).map(|entry| entry.definition),
            phase.repeats.get_mut(index),
        ) else {
            continue;
        };
        if running {
            state.blocked = true;
            record(
                out,
                at,
                "violation",
                format!("Concurrent loop {}", repeat.id),
                run,
            );
            continue;
        }
        enter_repeat_after_jump_one(
            repeat,
            state,
            at,
            target - state.anchor.unwrap_or(0),
            out,
            run,
            history,
        )?;
        running = true;
    }
    Ok(())
}

fn enter_repeat_after_jump_one(
    repeat: &Loop,
    state: &mut RepeatState,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) -> Result<()> {
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
    let branch_count = i64::try_from(repeat.branches.len()).unwrap_or(i64::MAX);
    let partial_branches = i64::try_from(
        repeat
            .branches
            .iter()
            .filter(|branch| branch.close <= position)
            .count(),
    )
    .unwrap_or(i64::MAX);
    ensure!(
        completed <= 1_000_000
            && completed
                .saturating_mul(event_count.saturating_add(branch_count))
                .saturating_add(partial_count)
                .saturating_add(partial_branches)
                <= 1_000_000,
        "Phase sync would skip more than one million loop iterations, events or branches in {}",
        repeat.id
    );

    let mut events: Vec<_> = repeat.events.iter().enumerate().collect();
    events.sort_by_key(|(_, event)| (event.at, event.order));
    for _ in 0..completed {
        for (_, event) in &events {
            record(out, at, "skipped", event.id.clone(), run);
        }
        for branch in &repeat.branches {
            history.insert(branch.id.clone(), None);
            record(out, at, "unresolved", branch.id.clone(), run);
        }
    }
    let mut clock = Clock {
        base_ms: at - position * 100,
        phase_instance: state.phase_instance,
        iteration: Some(u64::try_from(completed)? + 1),
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
    state.branches = repeat
        .branches
        .iter()
        .map(|_| BranchState::default())
        .collect();
    let previous_len = out.len();
    advance_nested_branches(repeat, state, at, false, out, run, history);
    for item in out.iter_mut().skip(previous_len) {
        if item.action == "unresolved" && item.at_ms < at {
            item.at_ms = at;
        }
    }
    Ok(())
}

fn correct_phase(
    phase: &mut PhaseState<'_>,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) -> Result<()> {
    enter_repeat_after_jump(phase, at, target, out, run, history)?;
    for state in &mut phase.repeats {
        if state.current.is_none() {
            state.zero_start = None;
        }
    }
    correct(
        &phase.definition.events,
        &mut phase.clock,
        at,
        target,
        out,
        run,
    );
    correct_paths(phase, at, target, out, run);
    for (entry, state) in phase.loops.iter().zip(&mut phase.repeats) {
        if let Some(path) = &mut state.exit {
            correct(
                &entry.definition.on_exit,
                &mut path.clock,
                at,
                target - path.anchor,
                out,
                run,
            );
        }
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

fn activate_repeat(definition: &Loop, state: &mut RepeatState, phase_base: i64) -> bool {
    let Some(anchor) = state.anchor else {
        return false;
    };
    state.current = Some(Clock {
        base_ms: phase_base + (anchor + definition.start) * 100,
        phase_instance: state.phase_instance,
        iteration: Some(1),
        not_before: state.not_before,
        zero_at: if definition.start == 0 {
            state.zero_start
        } else {
            None
        },
        ..Clock::default()
    });
    state.branches = definition
        .branches
        .iter()
        .map(|_| BranchState::default())
        .collect();
    true
}

fn start_repeats(
    phase: &mut PhaseState<'_>,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    let mut pending: Vec<_> = phase
        .loops
        .iter()
        .zip(&phase.repeats)
        .enumerate()
        .filter(|(_, (entry, state))| {
            !state.exited
                && !state.blocked
                && state.current.is_none()
                && repeat_start(phase.clock.base_ms, entry.definition, state)
                    .is_some_and(|start| due(start, at, inclusive))
        })
        .filter_map(|(index, (entry, state))| {
            repeat_start(phase.clock.base_ms, entry.definition, state).map(|start| (start, index))
        })
        .collect();
    pending.sort_unstable();
    let mut running = phase.repeats.iter().any(|state| state.current.is_some());
    for (start_ms, index) in pending {
        let (Some(definition), Some(state)) = (
            phase.loops.get(index).map(|entry| entry.definition),
            phase.repeats.get_mut(index),
        ) else {
            continue;
        };
        if running {
            state.blocked = true;
            record(
                out,
                start_ms,
                "violation",
                format!("Concurrent loop {}", definition.id),
                run,
            );
            continue;
        }
        running = activate_repeat(definition, state, phase.clock.base_ms);
    }
}

// Each iteration owns fresh branch state; earlier selections remain in fight history.
fn advance_nested_branches(
    definition: &Loop,
    state: &mut RepeatState,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) {
    let Some(clock) = &state.current else {
        return;
    };
    let base = clock.base_ms;
    let time = elapsed(base, at);
    for (branch, branch_state) in definition.branches.iter().zip(&mut state.branches) {
        if !branch_state.opened && time >= branch.open {
            branch_state.opened = true;
            history.insert(branch.id.clone(), None);
        }
        if branch_state.opened && !branch_state.closed && time >= branch.close {
            branch_state.closed = true;
            if branch_state.selected.is_none() {
                record(
                    out,
                    (base + branch.close * 100 - 50).max(0),
                    "unresolved",
                    branch.id.clone(),
                    run,
                );
            }
        }
        for (events, path) in branch_state.paths_mut(branch) {
            emit(events, &mut path.clock, at, inclusive, out, run);
        }
    }
}

fn correct_repeat(
    definition: &Loop,
    state: &mut RepeatState,
    at: i64,
    target: Tick,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) {
    let Some(clock) = &mut state.current else {
        return;
    };
    correct(&definition.events, clock, at, target, out, run);
    for (branch, branch_state) in definition.branches.iter().zip(&mut state.branches) {
        for (events, path) in branch_state.paths_mut(branch) {
            correct(events, &mut path.clock, at, target - path.anchor, out, run);
        }
    }
    let previous_len = out.len();
    advance_nested_branches(definition, state, at, false, out, run, history);
    for item in out.iter_mut().skip(previous_len) {
        if item.action == "unresolved" && item.at_ms < at {
            item.at_ms = at;
        }
    }
}

// Close each completed iteration before emitting the next iteration's events.
fn advance_repeat(
    definition: &Loop,
    state: &mut RepeatState,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) {
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
    let Some(clock) = &state.current else {
        return;
    };
    let period = definition.every * 100;
    let mut boundary = clock.base_ms + period;
    while due(boundary, at, inclusive) {
        advance_nested_branches(definition, state, boundary, false, out, run, history);
        let Some(clock) = &mut state.current else {
            return;
        };
        emit(
            &definition.events,
            clock,
            clock.base_ms + period,
            false,
            out,
            run,
        );
        clock.base_ms += period;
        clock.iteration = clock.iteration.map(|iteration| iteration + 1);
        clock.zero_at = None;
        clock.processed.clear();
        boundary = clock.base_ms + period;
        state.branches = definition
            .branches
            .iter()
            .map(|_| BranchState::default())
            .collect();
    }
    advance_nested_branches(definition, state, at, inclusive, out, run, history);
    let Some(clock) = &mut state.current else {
        return;
    };
    emit(&definition.events, clock, at, inclusive, out, run);
}

fn advance_repeats(
    phase: &mut PhaseState<'_>,
    at: i64,
    inclusive: bool,
    out: &mut Vec<Record>,
    run: usize,
    history: &mut BTreeMap<String, Option<String>>,
) {
    start_repeats(phase, at, inclusive, out, run);
    for (entry, state) in phase.loops.iter().zip(&mut phase.repeats) {
        advance_repeat(entry.definition, state, at, inclusive, out, run, history);
    }
}

// Report a missed finite exit only after all alternative windows have ended.
fn repeat_unresolved_one(
    definition: &Loop,
    state: &mut RepeatState,
    base_ms: i64,
    at: i64,
    corrected: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
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
    if elapsed(base_ms, at) <= end {
        return;
    }
    state.unresolved = true;
    let ended_at = (base_ms + (end + 1) * 100 - 50).max(0);
    record(
        out,
        if corrected { at } else { ended_at },
        "unresolved",
        definition.id.clone(),
        run,
    );
}

fn repeat_unresolved(
    phase: &mut PhaseState<'_>,
    at: i64,
    corrected: bool,
    out: &mut Vec<Record>,
    run: usize,
) {
    for (entry, state) in phase.loops.iter().zip(&mut phase.repeats) {
        let Some(anchor) = state.anchor else {
            continue;
        };
        repeat_unresolved_one(
            entry.definition,
            state,
            phase.clock.base_ms + anchor * 100,
            at,
            corrected,
            out,
            run,
        );
    }
}

fn unobserved_repeat(phase: &PhaseState<'_>, at: i64, out: &mut Vec<Record>, run: usize) {
    for (entry, state) in phase.loops.iter().zip(&phase.repeats) {
        unobserved_repeat_one(entry.definition, state, at, out, run);
    }
}

fn unobserved_repeat_one(
    definition: &Loop,
    state: &RepeatState,
    at: i64,
    out: &mut Vec<Record>,
    run: usize,
) {
    if let Some(clock) = &state.current {
        unobserved(&definition.events, clock, at, run, out);
        for (branch, branch_state) in definition.branches.iter().zip(&state.branches) {
            for (events, path) in branch_state.paths(branch) {
                unobserved(events, &path.clock, at, run, out);
            }
        }
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
    phase_entries: usize,
    out: Vec<Record>,
}

fn nested_actions_for_loop(
    loop_index: usize,
    definition: &Loop,
    state: &RepeatState,
    observation: &Observation,
    history: &BTreeMap<String, Option<String>>,
) -> (Vec<BranchAction>, Vec<String>) {
    let Some(clock) = &state.current else {
        return (Vec::new(), Vec::new());
    };
    let time = elapsed(clock.base_ms, observation.at_ms);
    let mut actions = Vec::new();
    let mut unresolved = Vec::new();
    for (index, (branch, branch_state)) in
        definition.branches.iter().zip(&state.branches).enumerate()
    {
        if !branch_state.opened || branch_state.merge.is_some() || time < branch.open {
            continue;
        }
        if branch
            .merge
            .as_ref()
            .is_some_and(|merge| matches_any(&merge.whens, observation, time))
        {
            actions.push(BranchAction::RepeatMerge(loop_index, index));
        }
        if branch_state.closed || time >= branch.close {
            continue;
        }
        let (choices, missing) =
            option_actions(branch, branch_state, index, time, observation, history);
        actions.extend(choices.into_iter().filter_map(|choice| match choice {
            BranchAction::Select(_, option) => {
                Some(BranchAction::RepeatSelect(loop_index, index, option))
            }
            BranchAction::Conflict(_) => Some(BranchAction::RepeatConflict(loop_index, index)),
            _ => None,
        }));
        unresolved.extend(missing);
    }
    (actions, unresolved)
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
            advance_repeats(
                phase,
                at,
                inclusive,
                &mut self.out,
                self.run,
                &mut self.history,
            );
        }
    }

    // Prepare a new iteration for a signal at its boundary without firing zero-time events.
    fn prepare_nested_boundary(&mut self, at: i64) -> Option<usize> {
        let Some(phase) = &mut self.phase else {
            return None;
        };
        let active = phase
            .repeats
            .iter()
            .position(|state| state.current.is_some());
        let index = active.or_else(|| {
            phase
                .loops
                .iter()
                .zip(&phase.repeats)
                .position(|(entry, state)| {
                    !state.exited
                        && !state.blocked
                        && state.current.is_none()
                        && repeat_start(phase.clock.base_ms, entry.definition, state) == Some(at)
                })
        })?;
        let (entry, state) = (phase.loops.get(index)?, phase.repeats.get_mut(index)?);
        if entry.definition.branches.is_empty() {
            return None;
        }
        if active.is_none() {
            if !activate_repeat(entry.definition, state, phase.clock.base_ms) {
                return None;
            }
        } else {
            let clock = state.current.as_mut()?;
            if clock.base_ms + entry.definition.every * 100 != at {
                return None;
            }
            clock.base_ms = at;
            clock.zero_at = None;
            clock.processed.clear();
            state.branches = entry
                .definition
                .branches
                .iter()
                .map(|_| BranchState::default())
                .collect();
        }
        advance_nested_branches(
            entry.definition,
            state,
            at,
            false,
            &mut self.out,
            self.run,
            &mut self.history,
        );
        Some(index)
    }

    fn phase_sync_at_nested_boundary(&self, observation: &Observation) -> bool {
        let Some(phase) = &self.phase else {
            return false;
        };
        if sync_targets(&phase.definition.events, &phase.clock, observation).is_empty()
            && branch_sync_targets(phase, observation).is_empty()
        {
            return false;
        }
        let at = observation.at_ms;
        phase
            .loops
            .iter()
            .zip(&phase.repeats)
            .any(|(entry, state)| {
                !entry.definition.branches.is_empty()
                    && !state.exited
                    && !state.blocked
                    && state.current.as_ref().map_or_else(
                        || repeat_start(phase.clock.base_ms, entry.definition, state) == Some(at),
                        |clock| clock.base_ms + entry.definition.every * 100 == at,
                    )
            })
    }

    fn phase_action_at_nested_boundary(&self, observation: &Observation) -> bool {
        let Some(phase) = &self.phase else {
            return false;
        };
        let at = observation.at_ms;
        let boundary = phase
            .loops
            .iter()
            .zip(&phase.repeats)
            .any(|(entry, state)| {
                !entry.definition.branches.is_empty()
                    && !state.exited
                    && !state.blocked
                    && state.current.as_ref().map_or_else(
                        || repeat_start(phase.clock.base_ms, entry.definition, state) == Some(at),
                        |clock| clock.base_ms + entry.definition.every * 100 == at,
                    )
            });
        boundary
            && self.branch_actions(observation).0.iter().any(|action| {
                matches!(
                    action,
                    BranchAction::Select(..) | BranchAction::Merge(..) | BranchAction::Conflict(..)
                )
            })
    }

    fn repeat_matches(&self, observation: &Observation) -> Vec<usize> {
        let Some(phase) = &self.phase else {
            return Vec::new();
        };
        let time = elapsed(phase.clock.base_ms, observation.at_ms);
        phase
            .loops
            .iter()
            .zip(&phase.repeats)
            .enumerate()
            .filter(|(_, (entry, state))| {
                !state.exited
                    && state.anchor.is_some_and(|anchor| {
                        time >= anchor
                            && matches_any(&entry.definition.untils, observation, time - anchor)
                    })
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn repeat_observation(&mut self, observation: &Observation, index: usize) -> bool {
        let Some(phase) = &mut self.phase else {
            return false;
        };
        let (Some(definition), Some(state)) = (
            phase.loops.get(index).map(|entry| entry.definition),
            phase.repeats.get_mut(index),
        ) else {
            return false;
        };
        let Some(anchor) = state.anchor else {
            return false;
        };
        let time = elapsed(phase.clock.base_ms, observation.at_ms) - anchor;
        if state.exited || time < 0 || !matches_any(&definition.untils, observation, time) {
            return false;
        }
        let iteration = state.current.as_ref().and_then(|clock| clock.iteration);
        state.exited = true;
        state.current = None;
        state.exit = Some(active_path(&phase.clock, observation.at_ms));
        if let Some(exit) = &mut state.exit {
            exit.clock.iteration = iteration;
        }
        record_control(
            &mut self.out,
            observation.at_ms,
            "until",
            definition.id.clone(),
            self.run,
            &phase.clock,
            Owner {
                loop_id: Some(definition.id.clone()),
                on_exit: true,
                ..self
                    .timeline
                    .catalog
                    .loops
                    .iter()
                    .find(|repeat| repeat.id == definition.id)
                    .expect("parsed loop has catalog entry")
                    .owner
                    .clone()
            },
        );
        if let Some(context) = self.out.last_mut().and_then(|row| row.context.as_mut()) {
            context.iteration = iteration;
        }
        true
    }

    fn control_observation(&mut self, observation: &Observation) -> bool {
        let repeats = self.repeat_matches(observation);
        if repeats.is_empty() {
            return self.branch_observation(observation);
        }
        let (actions, unresolved) = self.branch_actions(observation);
        for id in unresolved {
            record(
                &mut self.out,
                observation.at_ms,
                "unresolved",
                format!("{id}: missing prior choice"),
                self.run,
            );
        }
        if !actions.is_empty() || repeats.len() > 1 {
            record(
                &mut self.out,
                observation.at_ms,
                "ambiguous",
                "Multiple control actions match",
                self.run,
            );
            return true;
        }
        repeats
            .first()
            .is_some_and(|index| self.repeat_observation(observation, *index))
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
            if !state.opened || state.merge.is_some() || time < branch.open {
                continue;
            }
            if branch
                .merge
                .as_ref()
                .is_some_and(|merge| matches_any(&merge.whens, observation, time))
            {
                actions.push(BranchAction::Merge(index));
            }
            if state.closed || time >= branch.close {
                continue;
            }
            let (candidates, missing) =
                option_actions(branch, state, index, time, observation, &self.history);
            actions.extend(candidates);
            unresolved.extend(missing);
        }
        let (nested, missing) = self.nested_branch_actions(observation);
        actions.extend(nested);
        unresolved.extend(missing);
        (actions, unresolved)
    }

    fn nested_branch_actions(&self, observation: &Observation) -> (Vec<BranchAction>, Vec<String>) {
        let Some(phase) = &self.phase else {
            return (Vec::new(), Vec::new());
        };
        let mut actions = Vec::new();
        let mut unresolved = Vec::new();
        for (loop_index, (entry, state)) in phase.loops.iter().zip(&phase.repeats).enumerate() {
            let (nested, missing) = nested_actions_for_loop(
                loop_index,
                entry.definition,
                state,
                observation,
                &self.history,
            );
            actions.extend(nested);
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
        if matches!(
            action,
            BranchAction::RepeatSelect(..)
                | BranchAction::RepeatMerge(..)
                | BranchAction::RepeatConflict(..)
        ) {
            return self.nested_branch_observation(action, observation);
        }
        let phase = self.phase.as_mut().expect("branch actions require a phase");
        let (index, option) = match action {
            BranchAction::Select(index, option) => (*index, Some(*option)),
            BranchAction::Merge(index) | BranchAction::Conflict(index) => (*index, None),
            _ => return true,
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
                let path = active_path(&phase.clock, observation.at_ms);
                let anchor = path.anchor;
                state.option = Some(path);
                self.history
                    .insert(branch.id.clone(), Some(choice.id.clone()));
                record_control(
                    &mut self.out,
                    observation.at_ms,
                    "option",
                    format!("{}/{}", branch.id, choice.id),
                    self.run,
                    &phase.clock,
                    branch_owner(&phase.definition.id, &branch.id, Some(&choice.id)),
                );
                phase.activate_option_loops(index, option_index, anchor, observation.at_ms);
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
                state.merge = Some(active_path(&phase.clock, observation.at_ms));
                record_control(
                    &mut self.out,
                    observation.at_ms,
                    "merge",
                    branch.id.clone(),
                    self.run,
                    &phase.clock,
                    branch_owner(&phase.definition.id, &branch.id, None),
                );
                phase.cancel_option_loops(index);
            }
            BranchAction::Conflict(_) => record(
                &mut self.out,
                observation.at_ms,
                "ambiguous",
                format!("Conflicting option for {}", branch.id),
                self.run,
            ),
            _ => {}
        }
        true
    }

    fn nested_branch_observation(
        &mut self,
        action: &BranchAction,
        observation: &Observation,
    ) -> bool {
        let (loop_index, branch_index) = match action {
            BranchAction::RepeatSelect(loop_index, branch_index, _)
            | BranchAction::RepeatMerge(loop_index, branch_index)
            | BranchAction::RepeatConflict(loop_index, branch_index) => {
                (*loop_index, *branch_index)
            }
            _ => return false,
        };
        let Some(phase) = &mut self.phase else {
            return false;
        };
        let (Some(entry), Some(repeat_state)) = (
            phase.loops.get(loop_index),
            phase.repeats.get_mut(loop_index),
        ) else {
            return false;
        };
        let (Some(clock), Some(branch), Some(state)) = (
            repeat_state.current.as_ref(),
            entry.definition.branches.get(branch_index),
            repeat_state.branches.get_mut(branch_index),
        ) else {
            return false;
        };
        match action {
            BranchAction::RepeatSelect(_, _, option_index) => {
                let Some(option) = branch.options.get(*option_index) else {
                    return false;
                };
                state.selected = Some(*option_index);
                state.option = Some(active_path(clock, observation.at_ms));
                self.history
                    .insert(branch.id.clone(), Some(option.id.clone()));
                record_control(
                    &mut self.out,
                    observation.at_ms,
                    "option",
                    format!("{}/{}", branch.id, option.id),
                    self.run,
                    clock,
                    Owner {
                        phase: Some(phase.definition.id.clone()),
                        loop_id: Some(entry.definition.id.clone()),
                        branch: Some(branch.id.clone()),
                        option: Some(option.id.clone()),
                        ..Owner::default()
                    },
                );
            }
            BranchAction::RepeatMerge(_, _) => {
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
                state.merge = Some(active_path(clock, observation.at_ms));
                record_control(
                    &mut self.out,
                    observation.at_ms,
                    "merge",
                    branch.id.clone(),
                    self.run,
                    clock,
                    Owner {
                        phase: Some(phase.definition.id.clone()),
                        loop_id: Some(entry.definition.id.clone()),
                        branch: Some(branch.id.clone()),
                        ..Owner::default()
                    },
                );
            }
            BranchAction::RepeatConflict(_, _) => record(
                &mut self.out,
                observation.at_ms,
                "ambiguous",
                format!("Conflicting option for {}", branch.id),
                self.run,
            ),
            _ => return false,
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
            self.phase_entries += 1;
            let state = PhaseState::new(phase, at, self.phase_entries);
            record_control(
                &mut self.out,
                at,
                "phase",
                phase.id.clone(),
                self.run,
                &state.clock,
                Owner {
                    phase: Some(phase.id.clone()),
                    ..Owner::default()
                },
            );
            self.phase = Some(state);
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
            (SyncOwner::Phase(_), Some(phase), _) => correct_phase(
                phase,
                at,
                *target,
                &mut self.out,
                self.run,
                &mut self.history,
            )?,
            (SyncOwner::Fight, _, Some(clock)) => correct(
                &self.timeline.events,
                clock,
                at,
                *target,
                &mut self.out,
                self.run,
            ),
            (SyncOwner::Repeat(id), Some(phase), _) => {
                prepare_repeat_boundary(phase, id, at);
                let index = phase
                    .loops
                    .iter()
                    .position(|repeat| repeat.definition.id == *id);
                if let Some((repeat, state)) = index.and_then(|i| {
                    phase
                        .loops
                        .get(i)
                        .map(|entry| entry.definition)
                        .zip(phase.repeats.get_mut(i))
                }) {
                    correct_repeat(
                        repeat,
                        state,
                        at,
                        *target,
                        &mut self.out,
                        self.run,
                        &mut self.history,
                    );
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
        let first_signal_record = self.out.len();
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
        if lifecycle.is_none() && !enter && self.fight.is_some() {
            self.observe_control(observation)?;
        }
        for record in self.out.iter_mut().skip(first_signal_record) {
            if record.at_ms == at {
                record.source_event_index = observation.source_event_index;
            }
        }
        Ok(())
    }

    fn observe_control(&mut self, observation: &Observation) -> Result<()> {
        let exits = self.repeat_matches(observation);
        let has_nested = self.phase.as_ref().is_some_and(|phase| {
            phase
                .loops
                .iter()
                .any(|entry| !entry.definition.branches.is_empty())
        });
        let previous_history = (has_nested && exits.len() == 1).then(|| self.history.clone());
        let sync_snapshot = (self.phase_sync_at_nested_boundary(observation)
            || self.phase_action_at_nested_boundary(observation))
        .then(|| {
            let repeats = self
                .phase
                .as_ref()
                .map(|phase| phase.repeats.clone())
                .unwrap_or_default();
            (repeats, self.history.clone())
        });
        let prepared = self.prepare_nested_boundary(observation.at_ms);
        let branch_actions = self.branch_actions(observation).0;
        let phase_action_precedes_timer = prepared.is_some()
            && exits.is_empty()
            && matches!(
                branch_actions.as_slice(),
                [BranchAction::Select(..) | BranchAction::Merge(..) | BranchAction::Conflict(..)]
            );
        let mut sync_snapshot = sync_snapshot;
        if phase_action_precedes_timer && let Some((repeats, history)) = sync_snapshot.take() {
            if let Some(phase) = &mut self.phase {
                phase.repeats = repeats;
            }
            self.history = history;
        }
        let exits_without_branch = previous_history.is_some()
            && exits.first().copied() == prepared
            && branch_actions.is_empty();
        if self.control_observation(observation) {
            if let (true, Some(history)) = (exits_without_branch, previous_history) {
                self.history = history;
            }
            return Ok(());
        }
        if let Some((repeats, history)) = sync_snapshot {
            if let Some(phase) = &mut self.phase {
                phase.repeats = repeats;
            }
            self.history = history;
        }
        self.synchronize(observation)?;
        self.advance_branches(observation.at_ms, true);
        if let Some(phase) = &mut self.phase {
            repeat_unresolved(phase, observation.at_ms, true, &mut self.out, self.run);
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

    fn settle(&mut self, end_ms: i64) {
        self.advance(end_ms, true);
        self.advance_branches(end_ms, false);
        if let Some(phase) = &mut self.phase {
            repeat_unresolved(phase, end_ms, false, &mut self.out, self.run);
        }
    }

    fn finish(mut self, end_ms: i64) -> Vec<Record> {
        self.settle(end_ms);
        self.record_unobserved(end_ms);
        for row in &mut self.out {
            // Attach definition ownership after execution so identity metadata cannot alter path scheduling.
            if let Some((context, definition)) = row.context.as_mut().zip(
                row.event_order
                    .and_then(|order| self.timeline.catalog.events.get(&order)),
            ) {
                context.owner = definition.owner.clone();
            }
        }
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

fn validate_repeat_support(timeline: &Timeline, end_ms: i64) -> Result<()> {
    for phase in &timeline.phases {
        let repeats = phase.loops.iter().chain(
            phase
                .branches
                .iter()
                .flat_map(|branch| branch.options.iter().flat_map(|option| &option.loops)),
        );
        for repeat in repeats {
            if !repeat.branches.is_empty() {
                validate_nested_repeat(repeat, end_ms)?;
            }
            ensure!(
                end_ms / (repeat.every * 100) <= 1_000_000,
                "Recording spans more than one million possible loop iterations; shorten the recording"
            );
        }
    }
    Ok(())
}

fn validate_nested_repeat(repeat: &Loop, end_ms: i64) -> Result<()> {
    let cycles = end_ms / (repeat.every * 100);
    let branch_count = i64::try_from(repeat.branches.len()).unwrap_or(i64::MAX);
    ensure!(
        cycles.saturating_mul(branch_count) <= 1_000_000,
        "Recording spans more than one million possible loop branch instances; shorten the recording"
    );
    Ok(())
}

fn replay_until<'a>(timeline: &'a Timeline, input: &Recording, at_ms: i64) -> Result<Replay<'a>> {
    validate_recording(input)?;
    validate_repeat_support(timeline, at_ms)?;
    let mut state = Replay {
        timeline,
        fight: None,
        phase: None,
        history: BTreeMap::new(),
        run: 0,
        phase_entries: 0,
        out: Vec::new(),
    };
    let mut observations = input
        .observations
        .iter()
        .take_while(|observation| observation.at_ms <= at_ms)
        .peekable();
    while let Some(observation) = observations.next() {
        state.observe(observation)?;
        if observations
            .peek()
            .is_none_or(|next| next.at_ms != observation.at_ms)
        {
            state.advance(observation.at_ms, true);
        }
    }
    Ok(state)
}

pub(crate) fn replay(timeline: &Timeline, input: &Recording) -> Result<Vec<Record>> {
    Ok(replay_until(timeline, input, input.end_ms)?.finish(input.end_ms))
}

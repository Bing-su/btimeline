use super::forecast::{Alternative, Upcoming};
use super::{
    Branch,
    BranchState,
    Clock,
    Event,
    Loop,
    Owner,
    Phase,
    Replay,
    event_time,
    execution_context,
    repeat_start,
};

pub(super) struct Projection {
    pub events: Vec<Upcoming>,
    pub alternatives: Vec<Alternative>,
    pub truncated: bool,
    pub reason: Option<&'static str>,
    at: i64,
    until: i64,
    run: usize,
    remaining: usize,
}

impl Projection {
    fn finish(mut self, max_rows: usize) -> Self {
        self.events.sort_by_key(|event| (event.at_ms, event.order));
        if self.events.len() > max_rows {
            self.events.truncate(max_rows);
            self.truncated = true;
            self.reason.get_or_insert("maxRows");
        }
        let mut remaining = max_rows;
        let mut counts = vec![0; self.alternatives.len()];
        let mut pending: Vec<usize> = (0..self.alternatives.len()).collect();
        while remaining > 0 && !pending.is_empty() {
            pending.retain(|&index| {
                *counts.get(index).expect("Option count exists")
                    < self
                        .alternatives
                        .get(index)
                        .expect("Option exists")
                        .events
                        .len()
            });
            for &index in &pending {
                let take = usize::from(remaining > 0);
                *counts.get_mut(index).expect("Option count exists") += take;
                remaining -= take;
            }
        }
        for (option, count) in self.alternatives.iter_mut().zip(counts) {
            option.omitted = option.events.len() - count;
            option.events.truncate(count);
            if option.omitted > 0 {
                self.truncated = true;
                self.reason.get_or_insert("maxRows");
            }
        }
        self
    }

    fn choices(&mut self, timeline: &super::Timeline, phase: &Phase, origin: i64, deadline: i64) {
        let Some(choice) = timeline
            .choices
            .iter()
            .find(|choice| choice.step_id == phase.id)
        else {
            return;
        };
        let at = origin + choice.at * 100;
        if at > self.until || at >= deadline {
            return;
        }
        for id in &choice.option_ids {
            if !self.spend() {
                return;
            }
            let option = timeline
                .phases
                .iter()
                .find(|phase| &phase.id == id)
                .expect("Choice option exists");
            let open = option
                .enters
                .iter()
                .map(|signal| at - signal.before * 100)
                .min()
                .unwrap_or(at);
            let close = option
                .enters
                .iter()
                .map(|signal| at + signal.after.unwrap_or(25) * 100)
                .max()
                .unwrap_or(at);
            let end = successor(timeline, id, at).map_or(deadline, |(_, end, _)| end);
            let committed = std::mem::take(&mut self.events);
            self.events(
                &option.events,
                &Clock {
                    base_ms: at,
                    scheduled: true,
                    ..Clock::default()
                },
                &Owner {
                    phase: Some(id.clone()),
                    ..Owner::default()
                },
                end,
            );
            let mut events = std::mem::replace(&mut self.events, committed);
            events.retain(|event| !event.id.contains("/shared/"));
            events.sort_by_key(|event| (event.at_ms, event.order));
            for event in &mut events {
                event.route = "unresolved";
            }
            self.alternatives.push(Alternative {
                choice_id: choice.id.clone(),
                option_id: id.clone(),
                at_ms: at,
                state: if self.at < open {
                    "waiting"
                } else if self.at < close {
                    "open"
                } else {
                    "unresolved"
                },
                events,
                omitted: 0,
            });
        }
    }

    fn spend(&mut self) -> bool {
        if self.remaining == 0 {
            self.truncated = true;
            self.reason = Some("budget");
            return false;
        }
        self.remaining -= 1;
        true
    }

    fn reactions(&mut self, active: &super::PhaseState<'_>, owner: &Owner, deadline: i64) {
        for (definition, clock) in active.definition.sequences.iter().zip(&active.sequences) {
            if !self.spend() {
                break;
            }
            if let Some(clock) = clock {
                self.events(
                    &definition.events,
                    clock,
                    &Owner {
                        sequence: Some(definition.id.clone()),
                        ..owner.clone()
                    },
                    deadline,
                );
            }
        }
    }

    fn events(&mut self, events: &[Event], clock: &Clock, owner: &Owner, deadline: i64) {
        for (index, event) in events.iter().enumerate() {
            if !self.spend() {
                break;
            }
            let at = event_time(clock, event);
            if !event.visible
                || clock.processed.contains(&index)
                || at <= self.at
                || at > self.until
                || at >= deadline
            {
                continue;
            }
            let route = if event.id.contains("/shared/") {
                "common"
            } else if owner.option.is_some()
                || owner
                    .phase
                    .as_ref()
                    .is_some_and(|id| id.split('/').count() > 2)
            {
                "selected"
            } else {
                "common"
            };
            self.events.push(Upcoming {
                id: event.id.clone(),
                name: event.name.clone(),
                at_ms: at,
                lead_ms: at - self.at,
                duration_ms: event.duration * 100,
                run: self.run,
                context: execution_context(clock, owner.clone()),
                origin_basis: if clock.scheduled {
                    "scheduled"
                } else {
                    "observed"
                },
                route,
                order: event.order,
            });
        }
    }

    fn branches(
        &mut self,
        branches: &[Branch],
        states: &[BranchState],
        owner: &Owner,
        deadline: i64,
    ) {
        for (branch, state) in branches.iter().zip(states) {
            if !self.spend() {
                return;
            }
            let option = state
                .selected
                .and_then(|index| branch.options.get(index))
                .filter(|_| state.merge.is_none())
                .map(|option| option.id.clone());
            for (events, path) in state.paths(branch) {
                let owner = Owner {
                    branch: Some(branch.id.clone()),
                    option: option.clone(),
                    ..owner.clone()
                };
                self.events(events, &path.clock, &owner, deadline);
            }
        }
    }

    fn repeat(
        &mut self,
        definition: &Loop,
        first: Clock,
        branches: &[BranchState],
        owner: &Owner,
        deadline: i64,
    ) {
        let owner = Owner {
            loop_id: Some(definition.id.clone()),
            ..owner.clone()
        };
        let period = definition.every * 100;
        let mut clock = first;
        let mut initial = true;
        while clock.base_ms <= self.until && clock.base_ms < deadline {
            if !self.spend() {
                break;
            }
            let iteration = clock.iteration.unwrap_or(1);
            if definition
                .count
                .is_some_and(|count| iteration > count.unsigned_abs())
            {
                break;
            }
            let end = deadline.min(clock.base_ms + period);
            self.events(&definition.events, &clock, &owner, end);
            if initial {
                self.branches(&definition.branches, branches, &owner, end);
            }
            clock.base_ms += period;
            clock.iteration = Some(iteration + 1);
            clock.scheduled = true;
            clock.zero_at = None;
            clock.processed.clear();
            initial = false;
        }
    }
}

pub(super) fn successor<'a>(
    timeline: &'a super::Timeline,
    current: &str,
    origin: i64,
) -> Option<(&'a Phase, i64, super::Tick)> {
    timeline.phases.iter().find_map(|phase| {
        phase
            .after
            .iter()
            .find(|entry| entry.from == current)
            .map(|entry| (phase, origin + entry.at * 100, entry.at))
    })
}

pub(super) fn project(state: &Replay<'_>, until: i64, max_rows: usize) -> Projection {
    let mut result = Projection {
        events: Vec::new(),
        alternatives: Vec::new(),
        truncated: false,
        reason: None,
        at: state.at_ms,
        until,
        run: state.run,
        remaining: 10_000,
    };
    if state.suspended {
        return result;
    }
    if let Some(clock) = &state.fight {
        result.events(&state.timeline.events, clock, &Owner::default(), i64::MAX);
    }
    let Some(active) = &state.phase else {
        return result.finish(max_rows);
    };
    let next = successor(state.timeline, &active.definition.id, active.clock.base_ms);
    let deadline = next.map_or(i64::MAX, |(_, at, _)| at);
    let owner = Owner {
        phase: Some(active.definition.id.clone()),
        ..Owner::default()
    };
    result.events(&active.definition.events, &active.clock, &owner, deadline);
    result.choices(
        state.timeline,
        active.definition,
        active.clock.base_ms,
        deadline,
    );
    result.branches(
        &active.definition.branches,
        &active.branches,
        &owner,
        deadline,
    );
    result.reactions(active, &owner, deadline);
    for (entry, repeat) in active.loops.iter().zip(&active.repeats) {
        if !result.spend() {
            break;
        }
        if repeat.exited || repeat.blocked {
            continue;
        }
        let clock = repeat.current.clone().or_else(|| {
            repeat_start(active.clock.base_ms, entry.definition, repeat).map(|base_ms| Clock {
                base_ms,
                scheduled: true,
                iteration: Some(1),
                phase_instance: active.clock.phase_instance,
                ..Clock::default()
            })
        });
        let loop_owner = entry.owner.map_or_else(
            || owner.clone(),
            |(branch, option)| {
                let branch = active
                    .definition
                    .branches
                    .get(branch)
                    .expect("Loop owner branch exists");
                let option = branch
                    .options
                    .get(option)
                    .expect("Loop owner option exists");
                Owner {
                    branch: Some(branch.id.clone()),
                    option: Some(option.id.clone()),
                    ..owner.clone()
                }
            },
        );
        if let Some(clock) = clock {
            result.repeat(
                entry.definition,
                clock,
                &repeat.branches,
                &loop_owner,
                deadline,
            );
        }
    }
    project_successors(
        &mut result,
        state.timeline,
        next,
        active.clock.phase_instance,
    );
    result.finish(max_rows)
}

fn project_successors<'a>(
    result: &mut Projection,
    timeline: &'a super::Timeline,
    mut next: Option<(&'a Phase, i64, super::Tick)>,
    mut instance: usize,
) {
    while let Some((phase, origin, _)) = next.filter(|(_, origin, _)| *origin <= result.until) {
        if !result.spend() {
            break;
        }
        instance += 1;
        next = successor(timeline, &phase.id, origin);
        let deadline = next.map_or(i64::MAX, |(_, at, _)| at);
        let owner = Owner {
            phase: Some(phase.id.clone()),
            ..Owner::default()
        };
        let clock = Clock {
            base_ms: origin,
            scheduled: true,
            phase_instance: instance,
            ..Clock::default()
        };
        result.events(&phase.events, &clock, &owner, deadline);
        result.choices(timeline, phase, origin, deadline);
        for definition in &phase.loops {
            result.repeat(
                definition,
                Clock {
                    base_ms: origin + definition.start * 100,
                    iteration: Some(1),
                    ..clock.clone()
                },
                &[],
                &owner,
                deadline,
            );
        }
    }
}

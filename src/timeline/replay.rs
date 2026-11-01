use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{Event, MAX_MS, Phase, Tick, Timeline};

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
}

#[derive(Default)]
struct Clock {
    base_ms: i64,
    processed: BTreeSet<usize>,
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
            let at = clock.base_ms + e.at * 100;
            !clock.processed.contains(i) && (at < until || (inclusive && at == until))
        })
        .collect();
    pending.sort_by_key(|(i, event)| (event.at, *i));
    for (i, event) in pending {
        clock.processed.insert(i);
        record(
            out,
            clock.base_ms + event.at * 100,
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

struct Replay<'a> {
    timeline: &'a Timeline,
    fight: Option<Clock>,
    phase: Option<(&'a Phase, Clock)>,
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
        if let Some((phase, clock)) = self.phase.as_mut() {
            emit(&phase.events, clock, at, inclusive, &mut self.out, self.run);
        }
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
        let old_phase = self.phase.as_ref().map(|(p, _)| p.id.as_str());
        let time = self
            .phase
            .as_ref()
            .map_or(0, |(_, c)| elapsed(c.base_ms, observation.at_ms));
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
            }
        }
        record(&mut self.out, at, role, "", self.run);
    }

    fn enter_phase(&mut self, candidates: &[&'a Phase], at: i64) {
        if let [phase] = candidates {
            self.phase = Some((
                *phase,
                Clock {
                    base_ms: at,
                    ..Clock::default()
                },
            ));
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

    fn synchronize(&mut self, observation: &Observation) {
        let at = observation.at_ms;
        let mut targets = BTreeSet::new();
        if let Some(clock) = self.fight.as_ref() {
            targets.extend(
                sync_targets(&self.timeline.events, clock, observation)
                    .into_iter()
                    .map(|t| (None, t)),
            );
        }
        if let Some((phase, clock)) = self.phase.as_ref() {
            targets.extend(
                sync_targets(&phase.events, clock, observation)
                    .into_iter()
                    .map(|t| (Some(phase.id.as_str()), t)),
            );
        }
        if targets.len() > 1 {
            record(
                &mut self.out,
                at,
                "ambiguous",
                "Multiple sync destinations; clocks unchanged",
                self.run,
            );
            return;
        }
        let Some((owner, target)) = targets.first() else {
            return;
        };
        match (owner, self.phase.as_mut(), self.fight.as_mut()) {
            (Some(_), Some((phase, clock)), _) => {
                correct(&phase.events, clock, at, *target, &mut self.out, self.run)
            }
            (None, _, Some(clock)) => correct(
                &self.timeline.events,
                clock,
                at,
                *target,
                &mut self.out,
                self.run,
            ),
            _ => {}
        }
        record(
            &mut self.out,
            at,
            "sync",
            format!(
                "{} -> {:.1}s",
                owner.unwrap_or("fight"),
                *target as f64 / 10.0
            ),
            self.run,
        );
    }

    // Apply one observation as a transaction: control first, sync only when unconsumed, then events.
    fn observe(&mut self, observation: &Observation) {
        let at = observation.at_ms;
        self.advance(at, false);
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
            self.synchronize(observation);
        }
        self.advance(at, true);
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
        if let Some((phase, clock)) = self.phase.as_ref() {
            unobserved(&phase.events, clock, end_ms, self.run, &mut self.out);
        }
    }

    fn finish(mut self, end_ms: i64) -> Vec<Record> {
        self.advance(end_ms, true);
        self.record_unobserved(end_ms);
        // Stable ties retain transition order across independent fight and phase schedules.
        self.out.sort_by_key(|record| record.at_ms);
        self.out
    }
}

pub(crate) fn replay(timeline: &Timeline, input: &Recording) -> Result<Vec<Record>> {
    ensure!(
        timeline.controls.is_empty(),
        "Replay currently supports lifecycle, phase, event and sync only; unsupported: {}",
        timeline.controls.join(", ")
    );
    validate_recording(input)?;
    let mut state = Replay {
        timeline,
        fight: None,
        phase: None,
        run: 0,
        out: Vec::new(),
    };
    for observation in &input.observations {
        state.observe(observation);
    }
    Ok(state.finish(input.end_ms))
}

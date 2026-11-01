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

pub(crate) fn replay(timeline: &Timeline, input: &Recording) -> Result<Vec<Record>> {
    ensure!(
        timeline.controls.is_empty(),
        "Replay currently supports lifecycle, phase, event and sync only; unsupported: {}",
        timeline.controls.join(", ")
    );
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
    let mut out = Vec::new();
    let mut fight: Option<Clock> = None;
    let mut phase: Option<(&Phase, Clock)> = None;
    let mut run = 0;
    for observation in &input.observations {
        let at = observation.at_ms;
        if let Some(clock) = fight.as_mut() {
            emit(&timeline.events, clock, at, false, &mut out, run);
        }
        if let Some((p, clock)) = phase.as_mut() {
            emit(&p.events, clock, at, false, &mut out, run);
        }
        let old_phase = phase.as_ref().map(|(p, _)| p.id.as_str());
        let enter_candidates: Vec<_> = if fight.is_some()
            || timeline.lifecycle.get("start").is_some_and(|signals| {
                signals
                    .iter()
                    .any(|s| s.matches(&observation.kind, &observation.fields, 0))
            }) {
            timeline
                .phases
                .iter()
                .filter(|p| {
                    Some(p.id.as_str()) != old_phase
                        && p.enters.iter().any(|s| {
                            let from_ok = s
                                .from
                                .as_ref()
                                .is_none_or(|from| old_phase == Some(from.as_str()));
                            let time = phase.as_ref().map_or(0, |(_, c)| elapsed(c.base_ms, at));
                            from_ok && s.matches(&observation.kind, &observation.fields, time)
                        })
                })
                .collect()
        } else {
            Vec::new()
        };
        let lifecycle = ["zone-exit", "wipe", "finish", "boss-complete", "start"]
            .into_iter()
            .find(|role| {
                (*role != "start" || fight.is_none())
                    && timeline.lifecycle.get(*role).is_some_and(|signals| {
                        signals
                            .iter()
                            .any(|s| s.matches(&observation.kind, &observation.fields, 0))
                    })
            });
        let mut consumed = false;
        if let Some(role) = lifecycle {
            consumed = true;
            match role {
                "start" => {
                    run += 1;
                    fight = Some(Clock {
                        base_ms: at,
                        ..Clock::default()
                    });
                }
                "boss-complete" => {
                    phase = None;
                }
                _ => {
                    fight = None;
                    phase = None;
                }
            }
            record(&mut out, at, role, "", run);
        }
        let terminal = matches!(lifecycle, Some("zone-exit" | "wipe" | "finish"));
        if !terminal && fight.is_some() && !enter_candidates.is_empty() {
            consumed = true;
            if let [p] = enter_candidates.as_slice() {
                phase = Some((
                    *p,
                    Clock {
                        base_ms: at,
                        ..Clock::default()
                    },
                ));
                record(&mut out, at, "phase", p.id.clone(), run);
            } else {
                record(
                    &mut out,
                    at,
                    "ambiguous",
                    "Multiple phase destinations",
                    run,
                );
            }
        }
        if !consumed && fight.is_some() {
            let mut targets = BTreeSet::new();
            if let Some(clock) = fight.as_ref() {
                for event in &timeline.events {
                    for sync in &event.syncs {
                        if sync.matches(
                            &observation.kind,
                            &observation.fields,
                            elapsed(clock.base_ms, at),
                        ) && let Some(target) = sync.at
                        {
                            targets.insert((None, target));
                        }
                    }
                }
            }
            if let Some((p, clock)) = phase.as_ref() {
                for event in &p.events {
                    for sync in &event.syncs {
                        if sync.matches(
                            &observation.kind,
                            &observation.fields,
                            elapsed(clock.base_ms, at),
                        ) && let Some(target) = sync.at
                        {
                            targets.insert((Some(p.id.as_str()), target));
                        }
                    }
                }
            }
            if targets.len() > 1 {
                record(
                    &mut out,
                    at,
                    "ambiguous",
                    "Multiple sync destinations; clocks unchanged",
                    run,
                );
            } else if let Some((owner, target)) = targets.first() {
                if owner.is_some() {
                    if let Some((p, clock)) = phase.as_mut() {
                        correct(&p.events, clock, at, *target, &mut out, run);
                    }
                } else if let Some(clock) = fight.as_mut() {
                    correct(&timeline.events, clock, at, *target, &mut out, run);
                }
                record(
                    &mut out,
                    at,
                    "sync",
                    format!(
                        "{} -> {:.1}s",
                        owner.unwrap_or("fight"),
                        *target as f64 / 10.0
                    ),
                    run,
                );
            }
        }
        if let Some(clock) = fight.as_mut() {
            emit(&timeline.events, clock, at, true, &mut out, run);
        }
        if let Some((p, clock)) = phase.as_mut() {
            emit(&p.events, clock, at, true, &mut out, run);
        }
    }
    if let Some(clock) = fight.as_mut() {
        emit(&timeline.events, clock, input.end_ms, true, &mut out, run);
    }
    if let Some((p, clock)) = phase.as_mut() {
        emit(&p.events, clock, input.end_ms, true, &mut out, run);
    }
    for (events, clock) in fight
        .as_ref()
        .map(|c| (timeline.events.as_slice(), c))
        .into_iter()
        .chain(phase.as_ref().map(|(p, c)| (p.events.as_slice(), c)))
    {
        for (i, event) in events.iter().enumerate() {
            if !clock.processed.contains(&i) {
                record(&mut out, input.end_ms, "unobserved", event.id.clone(), run);
            }
        }
    }
    // Merge independent fight/phase schedules by raw time; stable ties retain processing order.
    out.sort_by_key(|record| record.at_ms);
    Ok(out)
}

//! An observed next anchor closes a body; timers alone cannot certify missing intermediate displays.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{AnchorPair, Support};
use crate::timeline::{Record, Timeline};

#[derive(Deserialize, Serialize)]
pub(in crate::evaluate) struct BodyGap {
    pub run: usize,
    pub phase_instance: usize,
    pub iteration: u64,
    pub id: String,
    pub at_ms: Option<i64>,
    pub reason: String,
    pub from_raw_index: usize,
    pub to_raw_index: usize,
}

#[derive(Default, Deserialize, Serialize)]
pub(in crate::evaluate) struct BodyCoverage {
    pub complete_display_bodies: usize,
    pub incomplete_display_bodies: usize,
    pub unverified_iterations: usize,
    pub exits_with_matched_on_exit: usize,
    pub first_gap: Option<BodyGap>,
}

fn same_iteration(row: &Record, pair: &AnchorPair) -> bool {
    row.run == pair.run
        && row.context.as_ref().is_some_and(|context| {
            context.phase_instance == pair.phase_instance
                && context.iteration == Some(pair.iteration)
        })
}

fn first_gap(
    model: &Timeline,
    id: &str,
    rows: &[Record],
    support: &[&Support<'_>],
    pair: &AnchorPair,
) -> Option<BodyGap> {
    let body = rows
        .iter()
        .filter(|row| {
            same_iteration(row, pair)
                && row.context.as_ref().is_some_and(|context| {
                    context.owner.loop_id.as_deref() == Some(id) && !context.owner.on_exit
                })
        })
        .collect::<Vec<_>>();
    let window = support
        .iter()
        .find(|sample| sample.raw_index == pair.from_raw_index)
        .zip(
            support
                .iter()
                .find(|sample| sample.raw_index == pair.to_raw_index),
        )
        .map(|(from, to)| from.observed_at_ms..=to.observed_at_ms);
    let matched = support
        .iter()
        .filter(|sample| body.iter().any(|row| std::ptr::eq(sample.record, *row)))
        .collect::<Vec<_>>();
    let missing_event = body
        .iter()
        .find(|row| {
            row.event.as_ref().is_some_and(|event| event.visible)
                && !matched.iter().any(|sample| {
                    std::ptr::eq(sample.record, **row)
                        && window
                            .as_ref()
                            .is_some_and(|range| range.contains(&sample.observed_at_ms))
                })
        })
        .and_then(|row| {
            row.event
                .as_ref()
                .map(|event| (event.id.clone(), Some(row.at_ms), "missing_display"))
        });
    // Preserve partial order while allowing packet reordering within a 100ms simultaneous wave.
    let reversed = matched
        .iter()
        .find(|later| {
            matched.iter().any(|earlier| {
                earlier.record.at_ms + 100 < later.record.at_ms
                    && earlier.observed_at_ms > later.observed_at_ms + 100
            })
        })
        .and_then(|sample| {
            sample.record.event.as_ref().map(|event| {
                (
                    event.id.clone(),
                    Some(sample.record.at_ms),
                    "observation_order",
                )
            })
        });
    let missing_choice = model
        .catalog
        .branches
        .iter()
        .filter(|branch| branch.owner.loop_id.as_deref() == Some(id))
        .find(|branch| {
            !body.iter().any(|row| {
                row.action == "option"
                    && row.source_event_index.is_some()
                    && row
                        .context
                        .as_ref()
                        .is_some_and(|context| context.owner.branch.as_deref() == Some(&branch.id))
            })
        })
        .map(|branch| (branch.id.clone(), None, "unobserved_choice"));
    missing_event
        .or(reversed)
        .or(missing_choice)
        .map(|(id, at_ms, reason)| BodyGap {
            run: pair.run,
            phase_instance: pair.phase_instance,
            iteration: pair.iteration,
            id,
            at_ms,
            reason: reason.into(),
            from_raw_index: pair.from_raw_index,
            to_raw_index: pair.to_raw_index,
        })
}

pub(super) fn collect(
    model: &Timeline,
    id: &str,
    rows: &[Record],
    support: &[&Support<'_>],
    pairs: &[AnchorPair],
    emitted_iterations: usize,
) -> BodyCoverage {
    let mut result = BodyCoverage {
        unverified_iterations: emitted_iterations.saturating_sub(pairs.len()),
        ..BodyCoverage::default()
    };
    for pair in pairs {
        if let Some(gap) = first_gap(model, id, rows, support, pair) {
            result.incomplete_display_bodies += 1;
            result.first_gap.get_or_insert(gap);
        } else {
            result.complete_display_bodies += 1;
        }
    }
    let exits = rows
        .iter()
        .filter(|row| {
            row.action == "until"
                && row
                    .context
                    .as_ref()
                    .is_some_and(|context| context.owner.loop_id.as_deref() == Some(id))
        })
        .filter_map(|row| row.source_event_index.map(|index| (row, index)));
    result.exits_with_matched_on_exit = exits
        .filter(|(exit, index)| {
            support.iter().any(|sample| {
                sample.raw_index > *index
                    && sample.record.at_ms >= exit.at_ms
                    && sample.record.run == exit.run
                    && sample
                        .record
                        .context
                        .as_ref()
                        .zip(exit.context.as_ref())
                        .is_some_and(|(context, exit_context)| {
                            context.owner.on_exit
                                && context.owner.loop_id.as_deref() == Some(id)
                                && context.phase_instance == exit_context.phase_instance
                        })
            })
        })
        .map(|(row, _)| {
            (
                row.run,
                row.context.as_ref().map(|context| context.phase_instance),
            )
        })
        .collect::<BTreeSet<_>>()
        .len();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Observation, Recording};

    fn analyze(case: &str) -> BodyCoverage {
        let branch = if case == "choice" {
            r#"branch "choice" open=0.5 close=3.0 { option "left" { when "Left"; }; option "right" { when "Right"; }; }"#
        } else {
            ""
        };
        let tail = if case == "simultaneous" { "1.0" } else { "2.0" };
        let model = crate::timeline::parse(&format!(
            r#"timeline 1 "Bodies" {{
            start "Begin"
            phase "boss" {{ enter "Begin"
                loop "rotation" start=0.0 every=4.0 {{
                    until "End" at=0.0 before=0.0
                    event "pulse" 0.0 name="Pulse"
                    event "middle" 1.0 name="Middle"
                    event "hidden" 1.5 name="Control" visible=#false
                    event "tail" {tail} name="Tail"
                    {branch}
                    on-exit {{ event "after" 1.0 name="After"; }}
                }}
            }}
        }}"#
        ))
        .expect("timeline");
        let input = Recording {
            version: 1,
            end_ms: 11000,
            observations: [(0, "Begin", 0), (9000, "End", 900)]
                .into_iter()
                .map(|(at_ms, kind, index)| Observation {
                    at_ms,
                    kind: kind.into(),
                    fields: Default::default(),
                    source_event_index: Some(index),
                })
                .collect(),
        };
        let rows = crate::timeline::replay(&model, &input).expect("replay");
        let support = rows
            .iter()
            .filter(|row| {
                row.event.as_ref().is_some_and(|event| {
                    event.visible
                        && !(case == "missing" && event.id == "middle" && row.at_ms == 1000)
                        && !(case == "no_exit" && event.id == "after")
                })
            })
            .map(|record| {
                let observed_at_ms = match (case, record.at_ms) {
                    ("order", 1000) => 2200,
                    ("order", 2000) => 1200,
                    ("simultaneous", 1000)
                        if record
                            .event
                            .as_ref()
                            .is_some_and(|event| event.id == "middle") =>
                    {
                        1020
                    }
                    _ => record.at_ms,
                };
                Support {
                    record,
                    raw_index: usize::try_from(observed_at_ms / 10).expect("index"),
                    observed_at_ms,
                }
            })
            .collect::<Vec<_>>();
        super::super::collect(
            &model,
            &rows,
            &support.iter().collect::<Vec<_>>(),
            &BTreeSet::from(["boss"]),
        )
        .loops
        .into_iter()
        .next()
        .expect("loop coverage")
        .body
        .expect("body coverage")
    }

    #[test]
    fn observed_boundaries_do_not_hide_a_missing_middle_or_reversed_order() {
        for (case, reason) in [
            ("missing", "missing_display"),
            ("order", "observation_order"),
            ("choice", "unobserved_choice"),
        ] {
            let result = analyze(case);
            assert_eq!(
                result.incomplete_display_bodies,
                if case == "choice" { 2 } else { 1 }
            );
            let gap = result.first_gap.expect("gap");
            assert_eq!(gap.reason, reason);
            assert_eq!((gap.from_raw_index, gap.to_raw_index), (0, 400));
        }
    }

    #[test]
    fn complete_bodies_allow_simultaneous_order_and_keep_terminal_iteration_unverified() {
        for case in ["complete", "simultaneous", "no_exit"] {
            let result = analyze(case);
            assert_eq!(result.complete_display_bodies, 2);
            assert_eq!(result.incomplete_display_bodies, 0);
            assert_eq!(result.unverified_iterations, 1);
            assert_eq!(
                result.exits_with_matched_on_exit,
                usize::from(case != "no_exit")
            );
        }
    }
}

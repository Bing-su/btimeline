//! Flag matched phase displays that recur at similar offsets between observed loop boundaries.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::AnchorPair;
use crate::evaluate::score::Support;
use crate::timeline::{LoopDefinition, Record};

#[derive(Deserialize, Serialize)]
pub(in crate::evaluate) struct Candidate {
    name: String,
    event_ids: [String; 2],
    raw_indices: [usize; 2],
    boundary_raw_indices: [usize; 3],
    offsets_ms: [i64; 2],
}

#[derive(Clone, Copy)]
struct Boundary {
    raw_index: usize,
    at_ms: i64,
}

pub(super) fn first_candidate(
    repeat: &LoopDefinition,
    rows: &[Record],
    support: &[&Support<'_>],
    pairs: &[AnchorPair],
) -> Option<Candidate> {
    let outside = support
        .iter()
        .copied()
        .filter(|sample| {
            sample.record.context.as_ref().is_some_and(|context| {
                context.owner.phase == repeat.owner.phase
                    && context.owner.loop_id.is_none()
                    && context.owner.branch.is_none()
            })
        })
        .collect::<Vec<_>>();
    if outside.len() < 2 {
        return None;
    }
    let observed = support
        .iter()
        .map(|sample| (sample.raw_index, sample.observed_at_ms))
        .collect::<BTreeMap<_, _>>();
    let mut boundaries = BTreeMap::new();
    for pair in pairs {
        for (iteration, raw_index) in [
            (pair.iteration, pair.from_raw_index),
            (pair.iteration + 1, pair.to_raw_index),
        ] {
            if let Some(&at_ms) = observed.get(&raw_index) {
                boundaries.insert(
                    (pair.run, pair.phase_instance, iteration),
                    Boundary { raw_index, at_ms },
                );
            }
        }
    }
    // Branch-only loops can still establish recurrence through the same signal-selected branch.
    if boundaries.is_empty() {
        let mut by_branch = BTreeMap::<&str, BTreeMap<_, _>>::new();
        for row in rows
            .iter()
            .filter(|row| row.action == "option" && row.resume_at_ms.is_none())
        {
            if let Some((context, raw_index)) = row.context.as_ref().zip(row.source_event_index)
                && context.owner.loop_id.as_deref() == Some(&repeat.id)
                && let Some((branch, iteration)) =
                    context.owner.branch.as_deref().zip(context.iteration)
            {
                by_branch.entry(branch).or_default().insert(
                    (row.run, context.phase_instance, iteration),
                    Boundary {
                        raw_index,
                        at_ms: row.at_ms,
                    },
                );
            }
        }
        return by_branch
            .values()
            .find_map(|boundaries| compare_intervals(&outside, boundaries));
    }
    compare_intervals(&outside, &boundaries)
}

fn compare_intervals(
    support: &[&Support<'_>],
    boundaries: &BTreeMap<(usize, usize, u64), Boundary>,
) -> Option<Candidate> {
    for (&(run, instance, iteration), first) in boundaries {
        let Some(second) = boundaries.get(&(run, instance, iteration + 1)) else {
            continue;
        };
        let Some(third) = boundaries.get(&(run, instance, iteration + 2)) else {
            continue;
        };
        let interval = |from: Boundary, to: Boundary| {
            support.iter().filter(move |sample| {
                let row = sample.record;
                row.run == run
                    && sample.observed_at_ms >= from.at_ms
                    && sample.observed_at_ms < to.at_ms
                    && row
                        .context
                        .as_ref()
                        .is_some_and(|context| context.phase_instance == instance)
            })
        };
        for a in interval(*first, *second) {
            let Some(event_a) = &a.record.event else {
                continue;
            };
            let offset_a = a.observed_at_ms - first.at_ms;
            // A small tolerance allows for cast jitter without merging distant slots.
            if let Some((b, event_b)) = interval(*second, *third)
                .filter_map(|sample| Some((sample, sample.record.event.as_ref()?)))
                .find(|(sample, event)| {
                    event_a.name == event.name
                        && offset_a.abs_diff(sample.observed_at_ms - second.at_ms) <= 500
                })
            {
                return Some(Candidate {
                    name: event_a.name.clone(),
                    event_ids: [event_a.id.clone(), event_b.id.clone()],
                    raw_indices: [a.raw_index, b.raw_index],
                    boundary_raw_indices: [first.raw_index, second.raw_index, third.raw_index],
                    offsets_ms: [offset_a, b.observed_at_ms - second.at_ms],
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Observation, Recording, parse, replay};

    #[rstest::rstest]
    #[case(false, 9000, true)]
    #[case(false, 7000, false)]
    #[case(true, 9000, false)]
    fn needs_two_observed_intervals_and_a_phase_owned_display(
        #[case] inside: bool,
        #[case] end_ms: i64,
        #[case] expected: bool,
    ) {
        let cues = if inside {
            r#"event "hit" 1.0 name="Hit""#
        } else {
            ""
        };
        let outside = if inside {
            ""
        } else {
            r#"event "hit-first" 1.0 name="Hit"
               event "hit-second" 5.0 name="Hit""#
        };
        let model = parse(&format!(
            r#"timeline 1 "Boundary" {{
            start "Pull"
            phase "boss" {{
                enter "Pull"
                {outside}
                loop "body" start=0.0 every=4.0 count=3 {{
                    event "anchor" 0.0 name="Anchor"
                    {cues}
                }}
            }}
        }}"#
        ))
        .unwrap();
        let rows = replay(
            &model,
            &Recording {
                version: 1,
                end_ms,
                observations: vec![Observation {
                    at_ms: 0,
                    kind: "Pull".into(),
                    fields: BTreeMap::new(),
                    source_event_index: None,
                }],
            },
        )
        .unwrap();
        let support = rows
            .iter()
            .filter(|row| row.action == "event")
            .enumerate()
            .map(|(raw_index, record)| Support {
                record,
                raw_index,
                observed_at_ms: record.at_ms,
            })
            .collect::<Vec<_>>();
        let coverage = super::super::collect(
            &model,
            &rows,
            &support.iter().collect::<Vec<_>>(),
            &std::collections::BTreeSet::from(["boss"]),
        );
        let candidate = &coverage.loops.first().unwrap().outside_body_candidate;
        assert_eq!(candidate.is_some(), expected);
        if let Some(candidate) = candidate {
            assert_eq!(candidate.name, "Hit");
            assert_eq!(candidate.offsets_ms, [1000, 1000]);
        }
    }
}

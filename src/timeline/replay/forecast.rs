use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{ExecutionContext, MAX_MS, Record, Recording, Timeline, replay_until};

#[derive(Serialize)]
pub(crate) struct Upcoming {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) at_ms: i64,
    pub(super) lead_ms: i64,
    pub(crate) duration_ms: i64,
    pub(crate) run: usize,
    pub(crate) context: ExecutionContext,
    pub(super) origin_basis: &'static str,
    pub(super) route: &'static str,
    #[serde(skip)]
    pub(super) order: usize,
}

#[derive(Serialize)]
pub(crate) struct Alternative {
    pub(super) choice_id: String,
    pub(super) option_id: String,
    pub(super) at_ms: i64,
    pub(super) state: &'static str,
    pub(super) events: Vec<Upcoming>,
    pub(super) omitted: usize,
}

#[derive(Serialize)]
pub(crate) struct Forecast {
    at_ms: i64,
    until_ms: i64,
    pub(crate) status: &'static str,
    pub(crate) phase: Option<String>,
    run: usize,
    observations_used: usize,
    pub(crate) events: Vec<Upcoming>,
    alternatives: Vec<Alternative>,
    pub(crate) diagnostics: Vec<Record>,
    pub(crate) projection_diagnostics: Vec<Record>,
    pub(crate) truncated: bool,
    pub(crate) truncation_reason: Option<&'static str>,
    assumptions: [&'static str; 2],
    not_checked: [&'static str; 3],
}

pub(crate) fn forecast(
    timeline: &Timeline,
    input: &Recording,
    at_ms: i64,
    horizon_ms: i64,
    max_rows: usize,
) -> Result<Forecast> {
    ensure!(
        (0..=input.end_ms).contains(&at_ms),
        "Forecast time must be inside the recording"
    );
    ensure!(horizon_ms >= 0, "Forecast horizon must be nonnegative");
    let until_ms = at_ms
        .checked_add(horizon_ms)
        .filter(|end| *end <= MAX_MS)
        .context("Forecast horizon exceeds the supported time range")?;
    let mut state = replay_until(timeline, input, at_ms)?;
    // Process every observation at the cutoff before looking ahead, e.g. a same-time wipe closes the phase first.
    state.settle(at_ms)?;
    let phase = state
        .phase
        .as_ref()
        .map(|phase| phase.definition.id.clone());
    let status = if state.suspended {
        "suspended"
    } else if state.fight.is_some() {
        "active"
    } else {
        "idle"
    };
    let run = state.run;
    let diagnostics = state
        .out
        .iter()
        .filter(|row| ["ambiguous", "violation", "unresolved"].contains(&row.action.as_str()))
        .cloned()
        .collect();
    let projection = super::projection::project(&state, until_ms, max_rows);
    Ok(Forecast {
        at_ms,
        until_ms,
        status,
        phase,
        run,
        observations_used: input
            .observations
            .iter()
            .take_while(|observation| observation.at_ms <= at_ms)
            .count(),
        events: projection.events,
        alternatives: projection.alternatives,
        diagnostics,
        projection_diagnostics: Vec::new(),
        truncated: projection.truncated,
        truncation_reason: projection.reason,
        assumptions: [
            "No observations after at_ms; the current path continues under authored timers",
            "Choice alternatives are conditional schedules, excluded from events; future observations are not predicted",
        ],
        not_checked: [
            "ACT signal equivalence",
            "game accuracy",
            "UI rendering and lookahead visibility",
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_alternatives_are_conditional_and_never_enter_the_scored_events() {
        let timeline = crate::timeline::parse(include_str!(
            "../../../tests/fixtures/timeline/shared-body.kdl"
        ))
        .expect("timeline");
        let recording = serde_json::from_value(serde_json::json!({
            "version": 1, "end_ms": 10000,
            "observations": [{ "at_ms": 0, "type": "Pull", "fields": {} }]
        }))
        .expect("recording");
        let result = forecast(&timeline, &recording, 1000, 9000, 100).expect("forecast");
        assert_eq!(result.alternatives.len(), 2);
        assert_eq!(
            result
                .events
                .iter()
                .map(|event| event.id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "fight/opening/choice/shared/common",
                "fight/continuation/joined"
            ]
        );
        for option in &result.alternatives {
            assert_eq!(option.events.len(), 1);
            assert_eq!(option.events[0].at_ms, 6000);
            assert_eq!(option.events[0].route, "unresolved");
            assert_eq!(option.events[0].origin_basis, "scheduled");
        }
        let missed = forecast(&timeline, &recording, 5500, 4000, 100).expect("missed");
        assert!(
            missed
                .alternatives
                .iter()
                .all(|option| option.state == "unresolved")
        );
        let bounded = forecast(&timeline, &recording, 1000, 9000, 1).expect("bounded");
        assert!(bounded.truncated);
        assert_eq!(
            bounded
                .alternatives
                .iter()
                .map(|option| option.events.len())
                .sum::<usize>(),
            1
        );
        assert_eq!(
            bounded
                .alternatives
                .iter()
                .map(|option| option.omitted)
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn selecting_a_choice_removes_alternatives_and_transfers_shared_ownership() {
        let timeline = crate::timeline::parse(include_str!(
            "../../../tests/fixtures/timeline/shared-body.kdl"
        ))
        .expect("timeline");
        let recording = serde_json::from_value(serde_json::json!({
            "version": 1, "end_ms": 10000,
            "observations": [
                { "at_ms": 0, "type": "Pull", "fields": {} },
                { "at_ms": 5100, "type": "Left", "fields": {} }
            ]
        }))
        .expect("recording");
        let result = forecast(&timeline, &recording, 5100, 4000, 100).expect("forecast");
        assert!(result.alternatives.is_empty());
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| event.id.contains("/shared/"))
                .count(),
            1
        );
        let selected = result
            .events
            .iter()
            .find(|event| event.id.ends_with("/left-hit"))
            .expect("left");
        assert_eq!(selected.at_ms, 6100);
        assert_eq!(selected.origin_basis, "observed");
    }

    #[test]
    fn selected_option_repeat_ignores_its_future_exit_and_merge() {
        let timeline = crate::timeline::parse(include_str!(
            "../../../tests/fixtures/timeline/option-loop.kdl"
        ))
        .expect("timeline");
        let recording = serde_json::from_str(include_str!(
            "../../../tests/fixtures/timeline/option-loop.json"
        ))
        .expect("recording");
        let result = forecast(&timeline, &recording, 1051, 3000, 10000).expect("forecast");
        assert_eq!(
            result
                .events
                .iter()
                .map(|event| (event.id.as_str(), event.at_ms, event.lead_ms))
                .collect::<Vec<_>>(),
            vec![
                ("boss/main/choice/left/loop/left-loop/pulse", 2051, 1000),
                ("boss/main/choice/left/loop/left-loop/pulse", 3051, 2000),
                ("boss/main/choice/left/loop/left-loop/pulse", 4051, 3000)
            ]
        );
        assert!(result.diagnostics.is_empty());
    }
}

use anyhow::Result;
use proptest::prelude::*;
use rstest::rstest;

use super::{Recording, parse, replay};

fn recording(end_ms: i64, observations: serde_json::Value) -> Result<Recording> {
    Ok(serde_json::from_value(
        serde_json::json!({"version":1,"end_ms":end_ms,"observations":observations}),
    )?)
}

#[rstest]
#[case::version("timeline 2 \"x\" {}")]
#[case::precision("timeline 1 \"x\" { event \"e\" 0.01 name=\"x\"; }")]
#[case::boolean("timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" visible=\"false\"; }")]
#[case::unknown_property("timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" typo=#true; }")]
#[case::duplicate_property("timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" name=\"y\"; }")]
#[case::duplicate_id(
    "timeline 1 \"x\" { event \"e\" 1.0 name=\"x\"; event \"e\" 2.0 name=\"y\"; }"
)]
#[case::unknown_phase("timeline 1 \"x\" { phase \"p\" { enter \"X\" from=\"missing\"; }; }")]
#[case::missing_from("timeline 1 \"x\" { phase \"p\" { enter \"X\" at=1.0; }; }")]
#[case::partial_window(
    "timeline 1 \"x\" { phase \"p\" { enter \"X\"; event \"e\" 1.0 name=\"x\" { sync 1.0 \"A\" before=1.0; }; }; }"
)]
#[case::duplicate_field(
    "timeline 1 \"x\" { phase \"p\" { enter \"X\" id=\"A\" { field \"id\" \"B\"; }; }; }"
)]
#[case::invalid_regex("timeline 1 \"x\" { phase \"p\" { enter \"X\" { regex \"id\" \"[\"; }; }; }")]
#[case::unknown_node("timeline 1 \"x\" { unknown; }")]
fn structure_rejects_invalid_documents(#[case] source: &str) {
    assert!(parse(source).is_err(), "Accepted {source}");
}

#[test]
fn branch_constraints_and_replay_capability_are_separate() {
    let body = r#"
    timeline 1 "Branches" {
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=20.0 {
                branch "choice" open=0.0 close=5.0 {
                    option "left" { when "A"; event "left" 3.0 name="Left"; }
                    option "right" { when "B"; event "right" 3.0 name="Right"; }
                    merge {
                        when "C" at=10.0 before=1.0 after=1.0
                        event "shared" 2.0 name="Shared"
                    }
                }
            }
        }
    }"#;
    let model = parse(body).unwrap();
    assert_eq!(model.event_count, 3);
    let input = recording(0, serde_json::json!([])).unwrap();
    replay(&model, &input).unwrap_err();
    for invalid in [
        body.replace("when \"C\" at=10.0 before=1.0 after=1.0", "when \"C\""),
        body.replace("at=10.0", "at=17.0"),
        body.replace(
            "event \"shared\" 2.0 name=\"Shared\"",
            "loop \"nested\" start=0.0 every=1.0 {}",
        ),
        body.replace(
            "event \"left\" 3.0 name=\"Left\"",
            "loop \"nested\" start=0.0 every=1.0 {}",
        ),
        body.replace("when \"A\";", "when \"A\"; requires \"choice\" \"right\";"),
        body.replace("when \"A\";", "when \"A\"; requires \"absent\" \"right\";"),
    ] {
        assert!(parse(&invalid).is_err(), "Accepted {invalid}");
    }
}

#[test]
fn lifecycle_cancels_same_time_events_and_restarts_fresh() {
    let model = parse(include_str!("../../tests/fixtures/timeline/basic.kdl")).unwrap();
    let input: Recording =
        serde_json::from_str(include_str!("../../tests/fixtures/timeline/basic.json")).unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("after-sync:") && r.at_ms == 6000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("cancelled-by-phase:"))
    );
    assert!(
        output.iter().any(|r| r.action == "event"
            && r.detail.starts_with("second-start:")
            && r.at_ms == 9000)
    );
    assert!(output.iter().any(|r| r.action == "start" && r.run == 2));
    assert!(output.iter().any(|r| r.action == "unobserved"
        && r.detail == "beyond-recording"
        && r.run == 1
        && r.at_ms == 10500));
    assert!(
        output
            .iter()
            .any(|r| r.action == "unobserved" && r.detail == "opening-hit" && r.run == 2)
    );
}

#[test]
fn raw_order_rounding_ambiguity_and_forward_backward_sync() {
    let source = r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "skipped" 2.0 name="Skipped"
            event "target" 4.0 name="Target" {
                sync 4.0 "Forward" before=3.0 after=3.0
                sync 1.0 "Backward" before=5.0 after=5.0
            }
            event "tail" 5.0 name="Tail"
        }
    }"#;
    let model = parse(source).unwrap();
    let input = recording(
        7000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Forward","fields":{}},
            {"at_ms":1040,"type":"Backward","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "skipped" && r.detail == "skipped")
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("target:"))
            .count(),
        1
    );
    assert_eq!(output.iter().filter(|r| r.action == "sync").count(), 2);
    assert!(!output.iter().any(|r| r.action == "ambiguous"));
    let ambiguous =
        parse(&source.replace("sync 1.0 \"Backward\"", "sync 1.0 \"Forward\"")).unwrap();
    assert!(
        replay(&ambiguous, &input)
            .unwrap()
            .iter()
            .any(|r| r.action == "ambiguous")
    );
    let out_of_order = recording(
        7000,
        serde_json::json!([
            {"at_ms":100,"type":"Start","fields":{}},
            {"at_ms":99,"type":"Backward","fields":{}}
        ]),
    )
    .unwrap();
    replay(&model, &out_of_order).unwrap_err();
}

#[test]
fn idle_enter_is_ignored_and_regex_matches_whole_fields() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Enter" { field "id" "A" "B"; regex "source" "Boss"; }
            event "e" 0.0 name="Entry"
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Enter","fields":{"id":"A","source":"Boss"}},
            {"at_ms":1000,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Enter","fields":{"id":"A","source":"Other Boss"}},
            {"at_ms":3000,"type":"Enter","fields":{"id":"B","source":"Boss"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let phase_times: Vec<_> = output
        .iter()
        .filter(|r| r.action == "phase")
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(phase_times, [3000]);
}

#[rstest]
#[case::backreference(r"(Boss)\1", "BossBoss", "Boss")]
#[case::lookahead(r"(?=Boss)Boss", "Boss", "Other Boss")]
#[case::alternative_backtracking(r"a|ab", "ab", "abc")]
#[case::final_newline(r"Boss", "Boss", "Boss\n")]
#[case::empty(r"", "", "x")]
fn ecmascript_patterns_match_whole_fields(
    #[case] pattern: &str,
    #[case] accepted: &str,
    #[case] rejected: &str,
) {
    // Exercise the parser's real wrapper so changes to capture numbering or anchors are visible.
    let model = parse(&format!(
        r##"timeline 1 "Pattern" {{
        start "Start" {{ regex "source" #"{pattern}"#; }}
        event "hit" 0.0 name="Hit"
    }}"##
    ))
    .unwrap();
    for (value, expected) in [(accepted, true), (rejected, false)] {
        let input = recording(
            0,
            serde_json::json!([
                {"at_ms":0,"type":"Start","fields":{"source":value}}
            ]),
        )
        .unwrap();
        assert_eq!(
            replay(&model, &input)
                .unwrap()
                .iter()
                .any(|r| r.action == "event"),
            expected
        );
    }
}

#[test]
fn source_based_post_adds_sync_moves_tulidisaster() {
    // Valigarmanda source offsets: 1800.0 -> 1803.9 -> 1816.0; observations are synthetic.
    let model = parse(
        r#"timeline 1 "Valigarmanda post-add excerpt" {
        start "Ability" id="9565"
        phase "after-adds" {
            enter "Ability" id="9565"
            event "tulidisaster-first" 16.0 name="Tulidisaster 1" {
                sync 3.9 "Ability" id="95B3" before=2.5 after=2.5
                sync 16.0 "Ability" id="8D67" before=2.5 after=2.5
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        318000,
        serde_json::json!([
            {"at_ms":300000,"type":"Ability","fields":{"id":"9565"}},
            {"at_ms":304900,"type":"Ability","fields":{"id":"95B3"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("tulidisaster-first:")
        && r.at_ms == 317000));
}

#[test]
fn quantized_window_boundaries_and_input_shape_are_strict() {
    let model = parse(
        r#"timeline 1 "Boundary" {
        start "Start"
        event "e" 2.0 name="Hit" { sync 1.0 "Signal" before=0.0 after=0.0; }
    }"#,
    )
    .unwrap();
    for (at_ms, matches) in [(949, false), (950, true), (1049, true), (1050, false)] {
        let input = recording(
            3000,
            serde_json::json!([
                {"at_ms":0,"type":"Start","fields":{}},
                {"at_ms":at_ms,"type":"Signal","fields":{}}
            ]),
        )
        .unwrap();
        assert_eq!(
            replay(&model, &input)
                .unwrap()
                .iter()
                .any(|r| r.action == "sync"),
            matches
        );
    }
    for invalid in [
        serde_json::json!({"version":1,"end_ms":0,"observations":[],"unknown":true}),
        serde_json::json!({"version":1,"end_ms":0,"observations":[{"at_ms":0,"type":"Start","fields":{"id":1}}]}),
    ] {
        serde_json::from_value::<Recording>(invalid).unwrap_err();
    }
    replay(&model, &recording(-1, serde_json::json!([])).unwrap()).unwrap_err();
}

proptest! {
    #[test]
    fn shifting_raw_timestamps_preserves_replay(offset in 0_i64..1_000_000_000) {
        let model = parse(include_str!("../../tests/fixtures/timeline/basic.kdl"))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let original: Recording = serde_json::from_str(include_str!("../../tests/fixtures/timeline/basic.json"))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let before = replay(&model, &original).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let mut shifted: serde_json::Value = serde_json::from_str(include_str!("../../tests/fixtures/timeline/basic.json"))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        shifted["end_ms"] = (11000 + offset).into();
        let observations = shifted["observations"].as_array_mut()
            .ok_or_else(|| TestCaseError::fail("Expected fixture observations"))?;
        for observation in observations {
            let at = observation["at_ms"].as_i64()
                .ok_or_else(|| TestCaseError::fail("Expected fixture timestamp"))?;
            observation["at_ms"] = (at + offset).into();
        }
        let shifted: Recording = serde_json::from_value(shifted)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let after = replay(&model, &shifted).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let expected: Vec<_> = before.iter().map(|r| (r.at_ms + offset, &r.action, &r.detail, r.run)).collect();
        let actual: Vec<_> = after.iter().map(|r| (r.at_ms, &r.action, &r.detail, r.run)).collect();
        prop_assert_eq!(actual, expected);
    }

    #[test]
    fn repeated_clock_corrections_never_execute_an_event_twice(
        corrections in prop::collection::vec((0_i64..500, any::<bool>()), 0..64),
    ) {
        let model = parse(r#"timeline 1 "Corrections" {
            start "Start"
            event "early" 2.0 name="Early"
            event "target" 4.0 name="Target" {
                sync 4.0 "Forward" before=1000.0 after=1000.0
                sync 1.0 "Backward" before=1000.0 after=1000.0
            }
            event "tail" 5.0 name="Tail"
        }"#).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let mut at = 0;
        let mut observations = vec![serde_json::json!({"at_ms":0,"type":"Start","fields":{}})];
        for (delay, forward) in corrections {
            at += delay;
            observations.push(serde_json::json!({
                "at_ms":at, "type":if forward {"Forward"} else {"Backward"}, "fields":{}
            }));
        }
        let end = at + 6000;
        let input = recording(end, observations.into()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let records = replay(&model, &input).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let mut seen = std::collections::BTreeSet::new();
        for record in &records {
            prop_assert!((0..=end).contains(&record.at_ms));
            if record.action == "event" {
                prop_assert!(seen.insert(&record.detail), "Duplicate event: {}", record.detail);
            }
        }
        prop_assert!(records.windows(2).all(|pair| pair.first().zip(pair.get(1)).is_some_and(|(a,b)| a.at_ms <= b.at_ms)));
    }
}

#[rstest]
#[case::same_signal("when \"Ability\" id=\"A\";", "when \"Ability\" id=\"A\";", true)]
#[case::different_ids("when \"Ability\" id=\"A\";", "when \"Ability\" id=\"B\";", false)]
#[case::different_types("when \"StartsUsing\" id=\"A\";", "when \"Ability\" id=\"A\";", false)]
#[case::one_unconstrained("when \"Ability\";", "when \"Ability\" id=\"A\";", true)]
#[case::different_sources(
    "when \"Ability\" id=\"A\" source=\"Boss\";",
    "when \"Ability\" id=\"A\" source=\"Add\";",
    false
)]
#[case::alternative_overlap(
    "when \"Ability\" { field \"id\" \"A\" \"B\"; };",
    "when \"Ability\" id=\"B\";",
    true
)]
#[case::regex_is_unknown(
    "when \"Ability\" { regex \"id\" \"A.*\"; };",
    "when \"Ability\" id=\"A001\";",
    false
)]
fn unconditional_literal_option_overlap(
    #[case] left: &str,
    #[case] right: &str,
    #[case] overlaps: bool,
) {
    let source = format!(
        r#"timeline 1 "Test" {{
        phase "p" {{
            enter "Start"
            branch "choice" open=0.0 close=10.0 {{
                option "left" {{ {left} }}
                option "right" {{ {right} }}
            }}
        }}
    }}"#
    );
    let result = parse(&source);
    if overlaps {
        assert!(format!("{:#}", result.unwrap_err()).contains("Conflicting unconditional options"));
    } else {
        assert!(result.is_ok(), "{result:?}");
    }
}

#[test]
fn matching_signals_with_mutually_exclusive_prior_choices_are_allowed() {
    let source = r#"timeline 1 "Choices" {
        phase "p" {
            enter "Start"
            branch "first" open=0.0 close=5.0 {
                option "left" { when "A"; }
                option "right" { when "B"; }
            }
            branch "second" open=6.0 close=10.0 {
                option "left" { requires "first" "left"; when "Ability" id="A"; }
                option "right" { requires "first" "right"; when "Ability" id="A"; }
            }
        }
    }"#;
    parse(source).unwrap();
}

#[rstest]
#[case::early(500)]
#[case::at_event(1000)]
fn wipe_records_censored_events_for_each_attempt(#[case] wipe_at: i64) {
    let model = parse(
        r#"timeline 1 "Wipes" {
        start "Start"
        wipe "Wipe"
        event "fight-hit" 1.0 name="Fight hit"
        phase "p" {
            enter "Start"
            event "phase-hit" 1.0 name="Phase hit"
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        wipe_at * 2 + 1,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":wipe_at,"type":"Wipe","fields":{}},
            {"at_ms":wipe_at+1,"type":"Start","fields":{}},
            {"at_ms":wipe_at*2+1,"type":"Wipe","fields":{}}
        ]),
    )
    .unwrap();
    let records = replay(&model, &input).unwrap();
    for run in [1, 2] {
        for id in ["fight-hit", "phase-hit"] {
            assert!(
                records
                    .iter()
                    .any(|r| r.run == run && r.action == "unobserved" && r.detail == id)
            );
            assert!(
                !records
                    .iter()
                    .any(|r| r.run == run && r.action == "event" && r.detail.starts_with(id))
            );
        }
    }
}

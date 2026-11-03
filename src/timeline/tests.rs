use anyhow::Result;
use proptest::prelude::*;
use rstest::rstest;

use super::{Recording, parse, replay};

fn recording(end_ms: i64, observations: serde_json::Value) -> Result<Recording> {
    Ok(serde_json::from_value(
        serde_json::json!({"version":1,"end_ms":end_ms,"observations":observations}),
    )?)
}

#[test]
fn occurrence_context_survives_sync_and_distinguishes_phase_reentry() {
    let model = parse(
        r#"timeline 1 "Identity" {
        start "Begin"
        phase "a" {
            enter "Begin"
            loop "rotation" start=0.0 every=4.0 {
                event "pulse" 0.0 name="Pulse" { sync 0.0 "Pulse"; }
            }
        }
        phase "b" { enter "Other"; event "other" 0.0 name="Other"; }
    }"#,
    )
    .unwrap();
    let input = recording(
        14000,
        serde_json::json!([
            {"at_ms":0,"type":"Begin","fields":{}}, {"at_ms":4000,"type":"Pulse","fields":{}},
            {"at_ms":8000,"type":"Pulse","fields":{}}, {"at_ms":9000,"type":"Other","fields":{}},
            {"at_ms":10000,"type":"Begin","fields":{}}, {"at_ms":14000,"type":"Pulse","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    let identities = rows
        .iter()
        .filter(|row| row.event.as_ref().is_some_and(|event| event.id == "pulse"))
        .filter_map(|row| {
            row.context
                .as_ref()
                .map(|context| (context.phase_instance, context.iteration))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        identities,
        vec![
            (1, Some(1)),
            (1, Some(2)),
            (1, Some(3)),
            (3, Some(1)),
            (3, Some(2))
        ]
    );
    let future = super::forecast(&model, &input, 10000, 5000).unwrap();
    assert!(future.events.iter().any(|event| event.id == "pulse"
        && event.context.phase_instance == 3
        && event.context.iteration == Some(2)));
}

#[test]
fn phase_jump_preserves_modeled_iteration_and_exit_retains_running_cycle() {
    let model = parse(
        r#"timeline 1 "Jump" {
        start "Begin"
        phase "a" {
            enter "Begin"
            event "advance" 20.0 name="Advance" visible=#false { sync 20.0 "Advance" before=25.0 after=25.0; }
            loop "rotation" start=10.0 every=4.0 {
                until "End" at=0.0 before=0.0
                event "pulse" 0.0 name="Pulse"
                on-exit { event "exit" 0.0 name="Exit"; }
            }
        }
    }"#,
    ).unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Begin","fields":{}},{"at_ms":1000,"type":"Advance","fields":{}},{"at_ms":4000,"type":"End","fields":{}}
        ]),
    ).unwrap();
    let rows = replay(&model, &input).unwrap();
    let pulse = rows
        .iter()
        .find(|row| row.event.as_ref().is_some_and(|event| event.id == "pulse"))
        .expect("jumped loop emits");
    assert_eq!(pulse.at_ms, 3000);
    assert_eq!(
        pulse.context.as_ref().expect("event context").iteration,
        Some(4)
    );
    for row in rows.iter().filter(|row| {
        row.action == "until" || row.event.as_ref().is_some_and(|event| event.id == "exit")
    }) {
        assert_eq!(
            row.context.as_ref().expect("exit context").iteration,
            Some(4)
        );
    }
    let future = super::forecast(&model, &input, 1000, 7000).unwrap();
    assert_eq!(
        future
            .events
            .iter()
            .filter(|event| event.id == "pulse")
            .map(|event| event.context.iteration)
            .collect::<Vec<_>>(),
        vec![Some(4), Some(5)]
    );
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
    let rotation = model.phases.first().unwrap().loops.first().unwrap();
    assert_eq!(rotation.id, "rotation");
    assert_eq!(rotation.branches.first().unwrap().options.len(), 2);
    let input = recording(0, serde_json::json!([])).unwrap();
    replay(&model, &input).unwrap();
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
fn loop_branch_selects_and_merges_independently_each_iteration() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; event "left-hit" 1.0 name="Left"; }
                    option "right" { when "B"; event "right-hit" 1.0 name="Right"; }
                    merge { when "Join" at=4.0 before=1.0 after=1.0; event "shared" 1.0 name="Shared"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        16000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":4000,"type":"Join","fields":{}},
            {"at_ms":10000,"type":"B","fields":{}},
            {"at_ms":14000,"type":"Join","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left" && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 10000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("right-hit:") && r.at_ms == 11000)
    );
    let shared: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("shared:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(shared, [5000, 15000]);
    assert!(!output.iter().any(|r| r.action == "unresolved"));
}

#[test]
fn branch_inside_option_loop_is_cancelled_on_outer_merge() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "outer" open=0.0 close=3.0 {
                option "left" {
                    when "A"
                    loop "rotation" start=0.0 every=10.0 {
                        branch "inner" open=0.0 close=2.0 {
                            option "one" { when "C"; event "hit" 0.0 name="Hit"; }
                            option "two" { when "D"; }
                        }
                    }
                }
                option "right" { when "B"; }
                merge { when "Merge"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":1000,"type":"C","fields":{}},
            {"at_ms":3000,"type":"Merge","fields":{}},
            {"at_ms":3500,"type":"D","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/one" && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("hit:") && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "merge" && r.detail == "outer")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/two")
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn phase_sync_can_start_a_nested_branch_in_an_option_loop() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 5.0 name="Anchor" { sync 5.0 "Sync" before=5.0 after=0.0; }
            branch "outer" open=0.0 close=2.0 {
                option "left" {
                    when "A"
                    loop "rotation" start=2.0 every=10.0 {
                        branch "inner" open=0.0 close=3.0 {
                            option "one" { when "C"; event "hit" 0.0 name="Hit"; }
                            option "two" { when "D"; }
                        }
                    }
                }
                option "right" { when "B"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":1500,"type":"Sync","fields":{}},
            {"at_ms":1500,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/one" && r.at_ms == 1500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("hit:") && r.at_ms == 1500)
    );
}

#[rstest]
#[case::option_first(true, "option-loop", "nested-loop")]
#[case::nested_first(false, "nested-loop", "option-loop")]
fn phase_choice_at_nested_start_preserves_document_order(
    #[case] option_first: bool,
    #[case] running: &str,
    #[case] blocked: &str,
) {
    let branch = r#"branch "outer" open=0.0 close=2.0 {
        option "left" { when "A"; loop "option-loop" start=0.0 every=1.0 { event "option-pulse" 0.0 name="Option"; }; }
        option "right" { when "B"; }
    }"#;
    let nested = r#"loop "nested-loop" start=1.0 every=10.0 {
        event "nested-pulse" 0.0 name="Nested"
        branch "inner" open=0.0 close=2.0 { option "one" { when "C"; }; option "two" { when "D"; }; }
    }"#;
    let body = if option_first {
        format!("{branch}; {nested};")
    } else {
        format!("{nested}; {branch};")
    };
    let source = format!(
        "timeline 1 \"x\" {{ start \"Start\"; phase \"p\" {{ enter \"Start\"; {body} }} }}"
    );
    let model = parse(&source).unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains(blocked))
    );
    let id = if running == "option-loop" {
        "option-pulse:"
    } else {
        "nested-pulse:"
    };
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with(id) && r.at_ms == 1000)
    );
}

#[test]
fn another_loop_can_exit_at_nested_loop_start_before_selection() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 { until "End" at=2.0 before=0.0; }
            loop "second" start=2.0 every=10.0 {
                branch "inner" open=0.0 close=2.0 {
                    option "one" { when "C"; }
                    option "two" { when "D"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"End","fields":{}},
            {"at_ms":2000,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "first" && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/one" && r.at_ms == 2000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn another_loop_exit_does_not_restore_old_nested_iteration_history() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            loop "rotation" start=0.0 every=2.0 {
                branch "choice" open=0.0 close=1.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
            loop "waiting" start=5.0 every=1.0 { until "Skip" at=2.0 before=0.0; }
        }
        phase "second" {
            enter "Next"
            branch "later" open=0.0 close=3.0 {
                option "left" { when "C"; requires "choice" "left"; }
                option "right" { when "D"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Skip","fields":{}},
            {"at_ms":2500,"type":"Next","fields":{}},
            {"at_ms":3000,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "waiting")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "later/left")
    );
}

#[test]
fn outer_merge_at_option_loop_start_preserves_previous_nested_choice() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            enter "Return"
            branch "outer" open=0.0 close=3.0 {
                option "left" {
                    when "A"
                    loop "rotation" start=1.0 every=10.0 {
                        branch "inner" open=0.0 close=1.0 {
                            option "one" { when "C"; }
                            option "two" { when "D"; }
                        }
                    }
                }
                option "right" { when "B"; }
                merge { when "Merge"; }
            }
        }
        phase "second" {
            enter "Next"
            branch "later" open=0.0 close=2.0 {
                option "one" { when "Check"; requires "inner" "one"; }
                option "two" { when "Other"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"A","fields":{}},
            {"at_ms":1600,"type":"C","fields":{}},
            {"at_ms":2000,"type":"Next","fields":{}},
            {"at_ms":3000,"type":"Return","fields":{}},
            {"at_ms":3500,"type":"A","fields":{}},
            {"at_ms":4500,"type":"Merge","fields":{}},
            {"at_ms":5000,"type":"Next","fields":{}},
            {"at_ms":5500,"type":"Check","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "merge" && r.detail == "outer" && r.at_ms == 4500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "later/one" && r.at_ms == 5500)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
}

#[test]
fn nested_option_sync_corrects_iteration_and_future_iterations() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 10.0 name="Phase"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" { when "A"; event "left-hit" 2.0 name="Left" { sync 2.0 "Sync" before=2.0 after=2.0; }; }
                    option "right" { when "B"; event "right-hit" 1.0 name="Right"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        11000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":9000,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "sync" && r.detail.starts_with("rotation") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 9000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("right-hit:") && r.at_ms == 10000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 10000)
    );
}

#[test]
fn phase_sync_does_not_move_an_active_loop_branch() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 6.0 name="Phase" { sync 5.0 "Sync" before=5.0 after=5.0; }
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=4.0 {
                    option "left" { when "A"; event "left-hit" 2.0 name="Left"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        10500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":1500,"type":"Sync","fields":{}},
            {"at_ms":10000,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 2500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-hit:") && r.at_ms == 3000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 10000)
    );
}

#[test]
fn phase_and_loop_branches_report_competing_choices() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "outer" open=0.0 close=3.0 {
                option "one" { when "A"; }
                option "two" { when "P"; }
            }
            loop "rotation" start=0.0 every=10.0 {
                branch "inner" open=0.0 close=3.0 {
                    option "one" { when "A"; }
                    option "two" { when "N"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":1500,"type":"N","fields":{}},
            {"at_ms":1600,"type":"P","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/two")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "outer/two")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail.ends_with("/one"))
    );
}

#[test]
fn phase_option_sync_does_not_shift_an_active_nested_branch() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "outer" open=0.0 close=3.0 {
                option "one" { when "O"; event "outer-hit" 4.0 name="Outer" { sync 4.0 "Sync" before=4.0 after=4.0; }; }
                option "two" { when "P"; }
            }
            loop "rotation" start=0.0 every=10.0 {
                branch "inner" open=2.0 close=4.0 {
                    option "one" { when "N"; }
                    option "two" { when "M"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        3500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"O","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":2500,"type":"N","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("outer-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "inner/one" && r.at_ms == 2500)
    );
}

#[test]
fn phase_sync_at_nested_loop_start_runs_before_the_start_timer() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 0.0 name="Anchor" { sync 0.0 "Sync" before=0.0 after=3.0; }
            loop "rotation" start=2.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":2500,"type":"A","fields":{}},
            {"at_ms":4000,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 4000)
    );
}

#[test]
fn phase_option_sync_at_nested_loop_start_runs_before_the_start_timer() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "outer" open=0.0 close=2.0 {
                option "one" { when "O"; event "anchor" 0.0 name="Anchor" { sync 0.0 "Sync" before=0.0 after=3.0; }; }
                option "two" { when "P"; }
            }
            loop "rotation" start=2.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"O","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":2500,"type":"A","fields":{}},
            {"at_ms":3500,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 3500)
    );
}

#[test]
fn phase_sync_skips_nested_iterations_without_guessing_choices() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            event "anchor" 25.0 name="Anchor" { sync 25.0 "Sync" before=25.0 after=0.0; }
            loop "rotation" start=3.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" { when "A"; event "left-hit" 0.0 name="Left"; }
                    option "right" { when "B"; }
                }
            }
        }
        phase "second" {
            enter "Next"
            branch "later" open=0.0 close=3.0 {
                option "left" { when "C"; requires "choice" "left"; }
                option "right" { when "D"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}},
            {"at_ms":1500,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Next","fields":{}},
            {"at_ms":2500,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "unresolved" && r.detail == "choice" && r.at_ms == 1000)
            .count(),
        2
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left" && r.at_ms == 1500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-hit:") && r.at_ms == 1500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "later/left")
    );
}

#[test]
fn phase_sync_past_the_current_nested_window_marks_it_unresolved() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 8.0 name="Anchor" { sync 8.0 "Sync" before=8.0 after=0.0; }
            loop "rotation" start=3.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}},
            {"at_ms":1500,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "choice" && r.at_ms == 1000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left")
    );
}

#[test]
fn separate_signal_at_phase_sync_timestamp_can_select_new_loop_branch() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 3.0 name="Anchor" { sync 3.0 "Sync" before=3.0 after=0.0; }
            loop "rotation" start=3.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; event "hit" 0.0 name="Hit"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left" && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("hit:") && r.at_ms == 1000)
    );
}

#[test]
fn nested_sync_skips_crossed_option_events() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=3.0 {
                    option "left" {
                        when "A"
                        event "early" 1.0 name="Early"
                        event "target" 4.0 name="Target" { sync 4.0 "Sync" before=4.0 after=4.0; }
                    }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":1500,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "skipped" && r.detail == "early" && r.at_ms == 1500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("target:") && r.at_ms == 1500)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("early:"))
    );
}

#[test]
fn nested_merge_sync_corrects_its_iteration_without_moving_the_phase() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 10.0 name="Phase"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                    merge { when "Join" at=3.0 before=1.0 after=1.0; event "shared" 1.0 name="Shared" { sync 1.0 "Sync" before=1.0 after=1.0; }; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        10500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":3000,"type":"Join","fields":{}},
            {"at_ms":3500,"type":"Sync","fields":{}},
            {"at_ms":9500,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("shared:") && r.at_ms == 3500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 9500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 10000)
    );
}

#[test]
fn forward_iteration_sync_reports_crossed_nested_window_at_sync_time() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                event "anchor" 6.0 name="Anchor" { sync 6.0 "Sync" before=6.0 after=0.0; }
                branch "choice" open=4.0 close=5.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "choice" && r.at_ms == 1000)
    );
}

#[test]
fn loop_exit_precedes_nested_option_sync_on_the_same_signal() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                until "Ability" id="X" at=2.0 before=1.0
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; event "hit" 2.0 name="Hit" { sync 2.0 "Ability" id="X" before=2.0 after=2.0; }; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"X"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "rotation" && r.at_ms == 2000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "sync" || (r.action == "event" && r.detail.starts_with("hit:")))
    );
}

#[test]
fn loop_sync_moves_nested_branch_window_and_does_not_reopen_closed_choices() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                event "anchor" 3.0 name="Anchor" { sync 3.0 "Forward" before=3.0 after=3.0; sync 0.0 "Back" before=0.0 after=9.0; }
                branch "choice" open=4.0 close=6.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#).unwrap();
    let input = recording(
        8000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Forward","fields":{}},
            {"at_ms":3000,"type":"A","fields":{}},
            {"at_ms":6000,"type":"Back","fields":{}},
            {"at_ms":7000,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left" && r.at_ms == 3000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right")
    );
}

#[rstest]
#[case::phase(false)]
#[case::iteration(true)]
fn backward_sync_before_branch_open_does_not_match_early_signals(#[case] nested: bool) {
    let branch = r#"branch "choice" open=1.0 close=4.0 {
        option "left" { when "A"; }
        option "right" { when "B"; }
    }"#;
    let anchor = r#"event "anchor" 0.0 name="Anchor" { sync 0.0 "Back" before=0.0 after=3.0; }"#;
    let body = if nested {
        format!("loop \"rotation\" start=0.0 every=10.0 {{ {anchor}; {branch}; }}")
    } else {
        format!("{anchor}; {branch};")
    };
    let source = format!(
        "timeline 1 \"x\" {{ start \"Start\"; phase \"p\" {{ enter \"Start\"; {body} }} }}"
    );
    let model = parse(&source).unwrap();
    let input = recording(
        3500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Back","fields":{}},
            {"at_ms":2500,"type":"A","fields":{}},
            {"at_ms":3000,"type":"B","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/right" && r.at_ms == 3000)
    );
}

#[test]
fn missing_new_iteration_choice_does_not_reuse_an_older_selection() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; }
                    option "right" { when "B"; }
                }
            }
        }
        phase "second" {
            enter "Next"
            branch "later" open=0.0 close=3.0 {
                option "left" { when "C"; requires "choice" "left"; }
                option "right" { when "D"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        14000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":12000,"type":"Next","fields":{}},
            {"at_ms":13000,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "choice")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "later/left")
    );
}

#[test]
fn loop_branch_can_require_a_choice_from_an_earlier_phase() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            branch "earlier" open=0.0 close=2.0 {
                option "left" { when "A"; }
                option "right" { when "B"; }
            }
        }
        phase "second" {
            enter "Next"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "C"; requires "earlier" "left"; }
                    option "right" { when "D"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Next","fields":{}},
            {"at_ms":2500,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "choice/left" && r.at_ms == 2500)
    );
}

#[test]
fn loop_exit_at_iteration_boundary_preserves_prior_choice_history() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "first" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                until "Ability" id="end" at=10.0 before=0.0
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "Ability" id="left"; }
                    option "right" { when "Ability" id="right"; }
                }
            }
        }
        phase "second" {
            enter "Next"
            branch "later" open=0.0 close=3.0 {
                option "left" { when "C"; requires "choice" "left"; }
                option "right" { when "D"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        13000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Ability","fields":{"id":"left"}},
            {"at_ms":10000,"type":"Ability","fields":{"id":"end"}},
            {"at_ms":11000,"type":"Next","fields":{}},
            {"at_ms":12000,"type":"C","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "rotation" && r.at_ms == 10000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "later/left")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "choice")
    );
}

#[test]
fn loop_exit_and_new_iteration_choice_on_same_observation_are_ambiguous() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                until "Ability" id="same" at=10.0 before=0.0
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "Ability" id="same"; }
                    option "right" { when "Ability" id="other"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        11000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":10000,"type":"Ability","fields":{"id":"same"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 10000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "until" || r.action == "option")
    );
}

#[test]
fn loop_exit_and_nested_merge_on_same_observation_are_ambiguous() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                until "Ability" id="same" at=4.0 before=0.0
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "Ability" id="left"; }
                    option "right" { when "Ability" id="right"; }
                    merge { when "Ability" id="same" at=4.0 before=1.0 after=1.0; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Ability","fields":{"id":"left"}},
            {"at_ms":4000,"type":"Ability","fields":{"id":"same"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 4000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "until" || r.action == "merge")
    );
}

#[test]
fn wipe_marks_active_nested_option_events_unobserved() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        wipe "Wipe"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=10.0 {
                branch "choice" open=0.0 close=2.0 {
                    option "left" { when "A"; event "left-hit" 2.0 name="Left"; }
                    option "right" { when "B"; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Wipe","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unobserved" && r.detail == "left-hit" && r.at_ms == 2000)
    );
}

#[test]
fn option_loop_preserves_exit_and_body() {
    let model = parse(
        r#"timeline 1 "x" {
        phase "p" {
            enter "Start"
            branch "b" open=0.0 close=3.0 {
                option "a" {
                    when "A"
                    loop "hold" start=1.0 every=5.0 {
                        until "End" at=2.0 before=1.0
                        event "pulse" 1.0 name="Pulse"
                        on-exit { event "end" 1.0 name="End"; }
                    }
                }
                option "b" { when "B"; }
            }
        }
    }"#,
    )
    .unwrap();
    let phase = model.phases.first().unwrap();
    let option = phase.branches.first().unwrap().options.first().unwrap();
    let hold = option.loops.first().unwrap();
    assert_eq!((hold.start, hold.every), (10, 50));
    assert_eq!(hold.untils.len(), 1);
    assert_eq!(hold.events.first().unwrap().id, "pulse");
    assert_eq!(hold.on_exit.first().unwrap().id, "end");
    replay(&model, &recording(0, serde_json::json!([])).unwrap()).unwrap();
}

#[test]
fn selected_option_loop_uses_phase_anchor_and_merge_cancels_it() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    loop "left-loop" start=0.0 every=1.0 {
                        until "End" at=2.0 before=0.0
                        event "left-pulse" 0.0 name="Left"
                        on-exit { event "left-exit" 0.0 name="Exit"; }
                    }
                }
                option "right" {
                    when "Right"
                    loop "right-loop" start=0.0 every=1.0 { event "right-pulse" 0.0 name="Right"; }
                }
                merge { when "Merge"; event "shared" 0.0 name="Shared"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1049,"type":"Left","fields":{}},
            {"at_ms":3000,"type":"Merge","fields":{}},
            {"at_ms":3100,"type":"End","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("left-pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1049, 2000]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("shared:") && r.at_ms == 3000)
    );
    assert!(!output.iter().any(|r| r.detail.starts_with("right-pulse")
        || r.detail.starts_with("left-exit")
        || r.action == "until"));
}

#[test]
fn rounded_up_selection_fires_zero_events_at_the_observation() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    event "zero" 0.0 name="Zero"
                    loop "hold" start=0.0 every=1.0 { event "pulse" 0.0 name="Pulse"; }
                }
                option "right" { when "Right"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1051,"type":"Left","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("zero:") && r.at_ms == 1051)
    );
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1051, 2100]);
}

#[test]
fn same_timestamp_sync_can_delay_new_option_zero_events() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 0.0 name="Anchor" { sync 0.0 "Sync" before=0.0 after=3.0; }
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    event "zero" 0.0 name="Zero"
                    loop "hold" start=0.0 every=1.0 { event "pulse" 0.0 name="Pulse"; }
                }
                option "right" { when "Right"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1051,"type":"Left","fields":{}},
            {"at_ms":1051,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    for id in ["zero:", "pulse:"] {
        assert!(
            output
                .iter()
                .any(|r| r.action == "event" && r.detail.starts_with(id) && r.at_ms == 2151)
        );
        assert!(
            !output
                .iter()
                .any(|r| r.action == "event" && r.detail.starts_with(id) && r.at_ms == 1051)
        );
    }
}

#[test]
fn rounded_up_exit_and_merge_fire_zero_events_at_their_signals() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    loop "hold" start=2.0 every=1.0 {
                        until "End" at=0.0 before=0.0
                        on-exit { event "exit-zero" 0.0 name="Exit"; }
                    }
                }
                option "right" { when "Right"; }
                merge { when "Merge"; event "merge-zero" 0.0 name="Merge"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":1051,"type":"End","fields":{}},
            {"at_ms":2051,"type":"Merge","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("exit-zero:") && r.at_ms == 1051)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("merge-zero:") && r.at_ms == 2051)
    );
}

#[test]
fn option_loop_can_exit_before_start_and_sync_its_exit_path() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 4.0 name="Phase"
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    loop "hold" start=4.0 every=2.0 {
                        until "End" at=1.0 before=0.0
                        event "pulse" 0.0 name="Pulse"
                        on-exit { event "exit-hit" 1.0 name="Exit" { sync 1.0 "Sync" before=1.0 after=1.0; }; }
                    }
                }
                option "right" { when "Right"; }
            }
        }
    }"#).unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":2000,"type":"End","fields":{}},
            {"at_ms":2500,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "hold" && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("exit-hit:") && r.at_ms == 2500)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 3500)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("pulse:"))
    );
}

#[test]
fn option_loop_starts_when_phase_loop_exits_at_the_same_boundary() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 { until "EndFirst" at=2.0 before=0.0; }
            branch "choice" open=0.0 close=2.0 {
                option "left" { when "Left"; loop "second" start=1.0 every=1.0 { event "pulse" 0.0 name="Pulse"; }; }
                option "right" { when "Right"; }
            }
        }
    }"#).unwrap();
    let input = recording(
        3000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":2000,"type":"EndFirst","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("pulse:") && r.at_ms == 2000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn simultaneous_option_and_phase_loop_starts_follow_document_order() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=2.0 {
                option "left" { when "Left"; loop "option-loop" start=0.0 every=1.0 { event "a" 0.0 name="A"; }; }
                option "right" { when "Right"; }
            }
            loop "phase-loop" start=1.0 every=1.0 { event "b" 0.0 name="B"; }
        }
    }"#).unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("a:") && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains("phase-loop") && r.at_ms == 1000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:"))
    );
}

#[test]
fn unselected_option_loop_does_not_block_a_phase_loop() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=2.0 {
                option "left" { when "Left"; loop "option-loop" start=0.0 every=1.0 { event "a" 0.0 name="A"; }; }
                option "right" { when "Right"; }
            }
            loop "phase-loop" start=1.0 every=1.0 { event "b" 0.0 name="B"; }
        }
    }"#).unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"Right","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:") && r.at_ms == 1000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "violation" || r.detail.starts_with("a:"))
    );
}

#[test]
fn merge_and_option_loop_exit_on_one_observation_are_ambiguous() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "left" { when "Left"; loop "hold" start=0.0 every=1.0 { until "X" at=1.0 before=0.0; }; }
                option "right" { when "Right"; }
                merge { when "X"; }
            }
        }
    }"#).unwrap();
    let input = recording(
        2500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":2000,"type":"X","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 2000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "until" || r.action == "merge")
    );
}

#[test]
fn phase_sync_can_skip_an_option_loop_start_without_losing_its_anchor() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 5.0 name="Anchor" { sync 5.0 "Forward" before=5.0 after=5.0; }
            branch "choice" open=0.0 close=3.0 {
                option "left" { when "Left"; loop "hold" start=2.0 every=2.0 { event "pulse" 0.0 name="Pulse"; }; }
                option "right" { when "Right"; }
            }
        }
    }"#).unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":1500,"type":"Forward","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "skipped" && r.detail == "pulse")
            .count(),
        1
    );
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1500, 3500]);
}

#[test]
fn merge_cancels_option_loop_exit_events() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "left" {
                    when "Left"
                    loop "hold" start=0.0 every=2.0 {
                        until "End" at=1.0 before=0.0
                        on-exit { event "later" 3.0 name="Later"; }
                    }
                }
                option "right" { when "Right"; }
                merge { when "Merge"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}},
            {"at_ms":2000,"type":"End","fields":{}},
            {"at_ms":3000,"type":"Merge","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "hold")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "merge" && r.detail == "choice")
    );
    assert!(!output.iter().any(|r| r.detail.starts_with("later")));
}

#[rstest]
#[case::before_start(1000, vec![], 2000)]
#[case::on_start(2000, vec![], 3000)]
#[case::after_two_pulses(4500, vec![2000, 3000, 4000], 5500)]
fn simple_loop_repeats_and_exits(
    #[case] until_ms: i64,
    #[case] pulses: Vec<i64>,
    #[case] exit_ms: i64,
) {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=2.0 every=1.0 {
                until "End" at=1.0 before=1.0
                event "pulse" 0.0 name="Pulse"
                on-exit { event "after" 1.0 name="After"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":until_ms,"type":"End","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let actual: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(actual, pulses);
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "rotation")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("after:") && r.at_ms == exit_ms)
    );
}

#[test]
fn finite_loop_exit_window_reports_unresolved_but_keeps_repeating() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=0.0 every=1.0 {
                until "End" at=1.0 before=0.0 after=0.0
                event "pulse" 0.0 name="Pulse"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2500,
        serde_json::json!([{ "at_ms":0, "type":"Start", "fields":{} }]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "rotation")
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
            .count(),
        3
    );
}

#[test]
fn excessive_loop_iterations_are_rejected() {
    let model = parse(
        r#"timeline 1 "x" { phase "p" { enter "Start"; loop "l" start=0.0 every=0.1 {}; }; }"#,
    )
    .unwrap();
    let input = recording(100_000_100, serde_json::json!([])).unwrap();
    assert!(
        replay(&model, &input)
            .unwrap_err()
            .to_string()
            .contains("one million")
    );
}

#[test]
fn sequential_loops_start_at_the_exit_boundary() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
                event "a" 0.0 name="A"
            }
            loop "second" start=2.0 every=1.0 {
                until "EndSecond" at=5.0 before=0.0
                event "b" 0.0 name="B"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"EndFirst","fields":{}},
            {"at_ms":5000,"type":"EndSecond","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let events: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event")
        .map(|r| (r.at_ms, r.detail.as_str()))
        .collect();
    assert_eq!(events.len(), 5);
    assert!(
        matches!(events.as_slice(), [(0, a), (1000, b), (2000, c), (3000, d), (4000, e)] if a.starts_with("a:") && b.starts_with("a:") && c.starts_with("b:") && d.starts_with("b:") && e.starts_with("b:"))
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn simultaneous_active_loops_report_violation_and_do_not_restart() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "second" start=1.0 every=1.0 { event "b" 0.0 name="B"; }
            loop "first" start=0.0 every=1.0 {
                until "EndFirst" at=3.0 before=0.0
                event "a" 0.0 name="A"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":3000,"type":"EndFirst","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains("second") && r.at_ms == 1000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:"))
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("a:"))
            .count(),
        3
    );
}

#[test]
fn simultaneous_loop_starts_use_document_order() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 { event "a" 0.0 name="A"; }
            loop "second" start=0.0 every=1.0 { event "b" 0.0 name="B"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        0,
        serde_json::json!([{ "at_ms":0, "type":"Start", "fields":{} }]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("a:") && r.at_ms == 0)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains("second") && r.at_ms == 0)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:"))
    );
}

#[test]
fn watching_and_exiting_a_future_loop_does_not_count_as_running() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "running" start=0.0 every=2.0 { event "pulse" 0.0 name="Pulse"; }
            loop "waiting" start=3.0 every=1.0 {
                until "Skip" at=1.0 before=0.0
                event "never" 0.0 name="Never"
                on-exit { event "after" 1.0 name="After"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Skip","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("after:") && r.at_ms == 2000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "violation" || r.detail.starts_with("never:"))
    );
}

#[test]
fn one_signal_matching_multiple_loop_exits_is_ambiguous() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "a" start=5.0 every=1.0 { until "Skip" at=1.0 before=0.0; }
            loop "b" start=6.0 every=1.0 { until "Skip" at=1.0 before=0.0; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Skip","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 1000)
    );
    assert!(!output.iter().any(|r| r.action == "until"));
}

#[test]
fn second_loop_sync_moves_only_its_iterations() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
                event "a" 0.0 name="A"
            }
            loop "second" start=2.0 every=2.0 {
                until "EndSecond" at=5.0 before=0.0
                event "b" 1.0 name="B" { sync 1.0 "Sync" before=1.0 after=1.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"EndFirst","fields":{}},
            {"at_ms":2500,"type":"Sync","fields":{}},
            {"at_ms":5000,"type":"EndSecond","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let b: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("b:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(b, [2500, 4500]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "sync" && r.detail.starts_with("second"))
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "second" && r.at_ms == 5000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn exit_sync_corrects_all_active_exit_paths() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=10.0 every=2.0 {
                until "EndFirst" at=1.0 before=0.0
                on-exit { event "a" 3.0 name="A" { sync 3.0 "Sync" before=2.0 after=2.0; }; }
            }
            loop "second" start=12.0 every=2.0 {
                until "EndSecond" at=2.0 before=0.0
                on-exit { event "b" 3.0 name="B"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"EndFirst","fields":{}},
            {"at_ms":2000,"type":"EndSecond","fields":{}},
            {"at_ms":3000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("a:") && r.at_ms == 3000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:") && r.at_ms == 4000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn phase_sync_skipping_multiple_loop_starts_blocks_the_later_loop() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 7.0 name="Anchor" { sync 7.0 "Sync" before=7.0 after=7.0; }
            loop "second" start=3.0 every=2.0 { event "b" 0.0 name="B"; }
            loop "first" start=2.0 every=2.0 { event "a" 0.0 name="A"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains("second") && r.at_ms == 1000)
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "skipped" && r.detail == "a")
            .count(),
        3
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:"))
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("a:") && r.at_ms == 2000)
    );
}

#[test]
fn second_loop_can_sync_at_the_same_timestamp_as_first_loop_exit() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=1.0 { until "EndFirst" at=2.0 before=0.0; }
            loop "second" start=2.0 every=2.0 {
                event "b" 0.0 name="B" { sync 0.0 "Sync" before=0.0 after=0.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"EndFirst","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "sync" && r.detail.starts_with("second") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("b:") && r.at_ms == 2000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn blocked_loop_cannot_sync_at_its_start_boundary() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "first" start=0.0 every=2.0 { event "a" 0.0 name="A"; }
            loop "second" start=1.0 every=2.0 {
                event "b" 0.0 name="B" { sync 0.0 "Sync" before=0.0 after=0.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "violation" && r.detail.contains("second"))
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "sync" || (r.action == "event" && r.detail.starts_with("b:")))
    );
}

#[test]
fn phase_and_loop_sync_matching_one_observation_are_ambiguous() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 3.0 name="Phase" { sync 1.0 "Sync" before=1.0 after=1.0; }
            loop "first" start=0.0 every=2.0 {
                event "a" 0.0 name="A" { sync 0.5 "Sync" before=1.0 after=1.0; }
            }
            loop "second" start=10.0 every=2.0 { event "b" 0.0 name="B"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 500)
    );
    assert!(!output.iter().any(|r| r.action == "sync"));
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 3000)
    );
}

#[test]
fn loop_and_branch_can_run_in_the_same_phase() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "b" open=0.0 close=3.0 {
                option "a" { when "Choose"; event "chosen" 2.0 name="Chosen"; }
                option "b" { when "Other"; }
            }
            loop "l" start=0.0 every=2.0 {
                until "Stop" at=3.0 before=0.0
                event "pulse" 0.0 name="Pulse"
                on-exit { event "after" 1.0 name="After"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Choose","fields":{}},
            {"at_ms":3000,"type":"Stop","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "b/a")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("chosen:") && r.at_ms == 3000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("after:") && r.at_ms == 4000)
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
            .count(),
        2
    );
}

#[test]
fn simultaneous_branch_choice_and_loop_exit_are_ambiguous() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "b" open=0.0 close=2.0 {
                option "a" { when "X"; }
                option "b" { when "Other"; }
            }
            loop "l" start=0.0 every=1.0 {
                until "X" at=1.0 before=1.0
                event "pulse" 0.0 name="Pulse"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"X","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "ambiguous" && r.at_ms == 1000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" || r.action == "until")
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
            .count(),
        3
    );
}

#[test]
fn option_sync_moves_phase_and_exit_window_without_moving_active_loop() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 4.0 name="Phase"
            branch "b" open=0.0 close=2.0 {
                option "a" {
                    when "Choose"
                    event "chosen" 2.0 name="Chosen" { sync 2.0 "Sync" before=2.0 after=2.0; }
                }
                option "b" { when "Other"; }
            }
            loop "l" start=0.0 every=2.0 {
                until "Stop" at=5.0 before=0.0 after=0.0
                event "pulse" 0.0 name="Pulse"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Choose","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":4000,"type":"Stop","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [0, 2000]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("chosen:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 3000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 4000)
    );
}

#[test]
fn loop_sync_moves_future_iterations_without_moving_phase() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 7.0 name="Phase"
            loop "rotation" start=2.0 every=4.0 {
                until "End" at=7.0 before=0.0 after=0.0
                event "pulse" 1.0 name="Pulse" { sync 1.0 "Sync" before=1.0 after=1.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        8000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2500,"type":"Sync","fields":{}},
            {"at_ms":7000,"type":"End","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [2500, 6500]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 7000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 7000)
    );
}

#[test]
fn loop_boundary_sync_precedes_zero_time_event() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=1.0 every=2.0 {
                event "pulse" 0.0 name="Pulse" { sync 0.0 "Sync" before=0.0 after=0.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}},
            {"at_ms":3000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "sync" && r.detail.starts_with("rotation"))
            .count(),
        2
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
            .count(),
        2
    );
}

#[test]
fn loop_exit_has_priority_over_iteration_sync() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            loop "rotation" start=1.0 every=2.0 {
                until "Stop" at=1.0 before=0.0
                event "pulse" 0.0 name="Pulse" { sync 0.0 "Stop" before=0.0 after=0.0; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Stop","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(output.iter().any(|r| r.action == "until"));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "sync" || r.action == "event")
    );
}

#[test]
fn exit_sync_shifts_phase_and_exit_events_together() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 4.0 name="Phase"
            loop "l" start=5.0 every=2.0 {
                until "Stop" at=1.0 before=1.0
                event "pulse" 0.0 name="Pulse"
                on-exit {
                    event "exit-hit" 2.0 name="Exit" { sync 2.0 "Sync" before=2.0 after=2.0; }
                }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Stop","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("exit-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 3000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("pulse:"))
    );
}

#[test]
fn phase_sync_does_not_move_an_active_iteration() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 6.0 name="Phase" { sync 5.0 "Sync" before=5.0 after=5.0; }
            loop "l" start=0.0 every=2.0 {
                until "Stop" at=7.0 before=0.0 after=0.0
                event "pulse" 0.0 name="Pulse"
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}},
            {"at_ms":3000,"type":"Stop","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [0, 2000]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 3000)
    );
}

#[test]
fn phase_sync_past_a_pending_loop_starts_the_current_iteration() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 5.0 name="Phase" { sync 5.0 "Sync" before=5.0 after=5.0; }
            loop "l" start=3.0 every=2.0 { event "pulse" 0.0 name="Pulse"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "skipped" && r.detail == "pulse")
            .count(),
        1
    );
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1000, 3000, 5000]);
}

#[test]
fn phase_sync_marks_skipped_iterations_and_partial_iteration_once() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 8.5 name="Anchor" { sync 8.5 "Forward" before=8.5 after=8.5; sync 1.0 "Back" before=20.0 after=20.0; }
            loop "l" start=3.0 every=2.0 {
                event "zero" 0.0 name="Zero"
                event "middle" 1.5 name="Middle"
            }
        }
    }"#).unwrap();
    let input = recording(
        2200,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Forward","fields":{}},
            {"at_ms":1100,"type":"Back","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let skipped: Vec<_> = output
        .iter()
        .filter(|r| r.action == "skipped" && (r.detail == "zero" || r.detail == "middle"))
        .map(|r| r.detail.as_str())
        .collect();
    assert_eq!(skipped, ["zero", "middle", "zero", "middle", "zero"]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("middle:") && r.at_ms == 1000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("zero:") && r.at_ms == 1500)
    );
}

#[test]
fn phase_sync_rejects_an_excessive_number_of_skipped_loop_iterations() {
    let model = parse(r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 1000000.0 name="Anchor" { sync 1000000.0 "Forward" before=1000000.0 after=0.0; }
            loop "l" start=1.0 every=0.1 { event "pulse" 0.0 name="Pulse"; }
        }
    }"#).unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Forward","fields":{}}
        ]),
    )
    .unwrap();
    assert!(
        replay(&model, &input)
            .unwrap_err()
            .to_string()
            .contains("one million")
    );
}

#[test]
fn phase_sync_moves_a_pending_loop_start() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 2.0 name="Anchor" { sync 2.0 "Sync" before=2.0 after=2.0; }
            loop "l" start=5.0 every=2.0 { event "pulse" 0.0 name="Pulse"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("pulse:") && r.at_ms == 4000)
    );
}

#[test]
fn phase_sync_at_loop_start_precedes_starting_the_loop() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 0.0 name="Anchor" { sync 0.0 "Sync" before=0.0 after=3.0; }
            loop "l" start=2.0 every=2.0 { event "pulse" 0.0 name="Pulse"; }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [4000]);
}

#[test]
fn phase_sync_reports_expired_exit_window_at_the_correction() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "anchor" 5.0 name="Anchor" { sync 5.0 "Sync" before=5.0 after=5.0; }
            loop "l" start=0.0 every=2.0 {
                until "Stop" at=2.0 before=0.0 after=0.0
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "l" && r.at_ms == 1000)
    );
}

#[test]
fn phase_branch_preserves_conditions_and_schedules_for_replay() {
    let model = parse(
        r#"timeline 1 "x" {
        phase "p" {
            enter "Start"
            branch "first" open=0.0 close=5.0 {
                option "left" { when "A" id="1"; event "left-hit" 1.0 name="Left"; }
                option "right" { when "B"; }
                merge { when "C" at=6.0 before=1.0 after=1.0; event "shared" 2.0 name="Shared"; }
            }
            branch "second" open=7.0 close=9.0 {
                option "one" { when "D"; requires "first" "left"; }
                option "two" { when "E"; }
            }
        }
    }"#,
    )
    .unwrap();
    let branches = &model.phases[0].branches;
    assert_eq!(branches.len(), 2);
    assert_eq!(branches[0].options[0].events[0].id, "left-hit");
    assert!(branches[0].options[0].whens[0].matches("A", &[("id".into(), "1".into())].into(), 0));
    assert_eq!(branches[0].merge.as_ref().unwrap().events[0].id, "shared");
    assert_eq!(
        branches[1].options[0].requires,
        [("first".into(), "left".into())]
    );
}

#[test]
fn branch_select_sync_merge_and_shared_clock() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "phase-hit" 5.0 name="Phase"
            branch "b" open=0.0 close=4.0 {
                option "left" {
                    when "Left"
                    event "left-zero" 0.0 name="Zero"
                    event "left-hit" 2.0 name="Left" { sync 2.0 "Sync" before=3.0 after=3.0; }
                    event "cancelled" 5.0 name="Cancelled"
                }
                option "right" { when "Right"; }
                merge { when "Merge"; event "shared" 1.0 name="Shared"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1049,"type":"Left","fields":{}},
            {"at_ms":2000,"type":"Sync","fields":{}},
            {"at_ms":3000,"type":"Merge","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "b/left")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-zero:") && r.at_ms == 1049)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("left-hit:") && r.at_ms == 2000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("phase-hit:") && r.at_ms == 4000)
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("shared:") && r.at_ms == 4000)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("cancelled:"))
    );
}

#[test]
fn prior_choices_survive_phase_transition_but_not_wipe() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        wipe "Wipe"
        phase "first" {
            enter "Start"
            branch "first-choice" open=0.0 close=2.0 {
                option "a" { when "A"; }
                option "b" { when "B"; }
            }
        }
        phase "second" {
            enter "Next"
            branch "second-choice" open=0.0 close=2.0 {
                option "a" { when "A"; requires "first-choice" "a"; }
                option "b" { when "B"; requires "first-choice" "b"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        8000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}},
            {"at_ms":2000,"type":"Next","fields":{}},
            {"at_ms":2500,"type":"A","fields":{}},
            {"at_ms":3000,"type":"Wipe","fields":{}},
            {"at_ms":4000,"type":"Start","fields":{}},
            {"at_ms":4500,"type":"Next","fields":{}},
            {"at_ms":5000,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.run == 1 && r.action == "option" && r.detail == "second-choice/a")
    );
    assert!(output.iter().any(|r| r.run == 2
        && r.action == "unresolved"
        && r.detail.contains("missing prior choice")));
    assert!(
        !output
            .iter()
            .any(|r| r.run == 2 && r.action == "option" && r.detail == "second-choice/a")
    );
}

#[test]
fn closed_branch_does_not_reopen_after_backward_sync() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            event "sync" 8.0 name="Sync" { sync 1.0 "Back" before=9.0 after=9.0; }
            branch "b" open=0.0 close=2.0 {
                option "a" { when "A"; }
                option "b" { when "B"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Back","fields":{}},
            {"at_ms":2100,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "b")
    );
    assert!(!output.iter().any(|r| r.action == "option"));
}

#[test]
fn competing_branch_actions_do_not_choose_or_merge() {
    let model = parse(
        r##"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "choice" open=0.0 close=3.0 {
                option "a" { when "X" { regex "id" "A.*"; }; }
                option "b" { when "X" id="ABC"; }
                merge { when "X" id="ABC"; }
            }
        }
    }"##,
    )
    .unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"X","fields":{"id":"ABC"}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(output.iter().any(|r| r.action == "ambiguous"));
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "choice")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" || r.action == "merge")
    );
}

#[rstest]
#[case(1949, true)]
#[case(1950, false)]
fn selection_close_uses_rounded_phase_time(#[case] at_ms: i64, #[case] selected: bool) {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "b" open=0.0 close=2.0 {
                option "a" { when "A"; }
                option "b" { when "B"; }
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":at_ms,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(output.iter().any(|r| r.action == "option"), selected);
    assert_eq!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "b"),
        !selected
    );
}

#[test]
fn simultaneous_events_across_branch_and_phase_follow_document_order() {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Start"
            branch "b" open=0.0 close=3.0 {
                option "a" { when "A"; event "branch-hit" 1.0 name="Branch"; }
                option "b" { when "B"; }
            }
            event "phase-hit" 2.0 name="Phase"
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        3000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"A","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let events: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event")
        .map(|r| r.detail.as_str())
        .collect();
    assert!(
        matches!(events.as_slice(), [first, second] if first.starts_with("branch-hit:") && second.starts_with("phase-hit:"))
    );
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
    fn forward_jump_skips_exactly_the_crossed_loop_events(target in 11_i64..200) {
        let seconds = format!("{}.{:01}", target / 10, target % 10);
        let source = format!(r#"timeline 1 "x" {{
            start "Start"
            phase "p" {{
                enter "Start"
                event "anchor" {seconds} name="Anchor" {{ sync {seconds} "Sync" before={seconds} after=0.0; }}
                loop "l" start=1.0 every=2.0 {{ event "pulse" 0.0 name="Pulse"; }}
            }}
        }}"#);
        let model = parse(&source).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let input = recording(1000, serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ])).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let output = replay(&model, &input).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let offset = target - 10;
        let expected = offset / 20 + i64::from(offset % 20 > 0);
        let skipped = output.iter().filter(|r| r.action == "skipped" && r.detail == "pulse").count();
        prop_assert_eq!(i64::try_from(skipped).unwrap_or(i64::MAX), expected);
        let at_target = output.iter().any(|r| r.action == "event" && r.detail.starts_with("pulse:") && r.at_ms == 1000);
        prop_assert_eq!(at_target, offset % 20 == 0);
    }

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
#[case::disjoint_windows(
    "when \"Ability\" id=\"A\" at=2.0 before=1.0 after=1.0;",
    "when \"Ability\" id=\"A\" at=5.0 before=1.0 after=1.0;",
    false
)]
#[case::windows_touch_at_inclusive_boundary(
    "when \"Ability\" id=\"A\" at=2.0 before=1.0 after=1.0;",
    "when \"Ability\" id=\"A\" at=4.0 before=1.0 after=1.0;",
    true
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
        assert!(format!("{:#}", result.unwrap_err()).contains("Conflicting options"));
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

#[test]
fn matching_signals_with_identical_prior_choices_are_rejected() {
    let source = r#"timeline 1 "Choices" {
        phase "p" {
            enter "Start"
            branch "first" open=0.0 close=5.0 {
                option "left" { when "A"; }
                option "right" { when "B"; }
            }
            branch "second" open=6.0 close=10.0 {
                option "left" { requires "first" "left"; when "Ability" id="A"; }
                option "right" { when "Ability" id="A"; requires "first" "left"; requires "first" "left"; }
            }
        }
    }"#;
    let error = parse(source).unwrap_err();
    assert!(format!("{error:#}").contains("identical prerequisites"));
}

#[rstest]
#[case::identical_requirements("left", true)]
#[case::conflicting_requirements("right", false)]
fn option_cannot_require_two_choices_of_the_same_branch(#[case] second: &str, #[case] valid: bool) {
    let source = format!(
        r#"timeline 1 "x" {{
        phase "p" {{
            enter "Start"
            branch "first" open=0.0 close=1.0 {{
                option "left" {{ when "A"; }}
                option "right" {{ when "B"; }}
            }}
            branch "next" open=1.0 close=2.0 {{
                option "one" {{ when "C"; requires "first" "left"; requires "first" "{second}"; }}
                option "two" {{ when "D"; }}
            }}
        }}
    }}"#
    );
    let result = parse(&source);
    assert_eq!(result.is_ok(), valid);
    if let Err(error) = result {
        assert!(format!("{error:#}").contains("conflicting choices for first"));
    }
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

#[test]
fn all_observations_at_a_timestamp_precede_scheduled_events() {
    let model = parse(
        r#"timeline 1 "Same timestamp" {
        start "Start"
        wipe "Wipe"
        phase "p" {
            enter "Start"
            event "hit" 1.0 name="Hit"
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Unrelated","fields":{}},
            {"at_ms":1000,"type":"Wipe","fields":{}}
        ]),
    )
    .unwrap();
    let records = replay(&model, &input).unwrap();
    assert!(
        !records
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("hit:"))
    );
    assert!(
        records
            .iter()
            .any(|r| r.action == "unobserved" && r.detail == "hit" && r.at_ms == 1000)
    );
}

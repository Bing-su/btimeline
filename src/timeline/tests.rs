use anyhow::Result;
use proptest::prelude::*;
use rstest::rstest;

use super::{Recording, parse, replay};

#[rstest]
#[case("B4D3")]
#[case("B4D7")]
#[case("B51F")]
#[case("B528")]
fn r12s_repeated_abilities_do_not_start_mid_combat(#[case] id: &str) {
    let model = parse(include_str!("../../timelines/r12s.kdl")).unwrap();
    let input = recording(
        260000,
        serde_json::json!([
            {"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1","isGameEntering":"0"}},
            {"at_ms":237000,"type":"Ability","fields":{"id":id,"source":"Lindwurm"}}
        ]),
    )
    .unwrap();
    assert!(replay(&model, &input).unwrap().is_empty());
}

#[rstest]
#[case("BEBD", "pre-checkpoint/main", 66.9)]
#[case("B505", "post-checkpoint/main", 180.3)]
#[case("B51C", "post-cosmic-kiss/main", 23.5)]
fn r12s_unique_anchors_resume_without_past_events(
    #[case] id: &str,
    #[case] phase: &str,
    #[case] checkpoint: f64,
) {
    let model = parse(include_str!("../../timelines/r12s.kdl")).unwrap();
    let input = recording(
        10001,
        serde_json::json!([
            {"at_ms":10000,"type":"Ability","fields":{"id":id,"source":"Lindwurm"}},
            {"at_ms":10001,"type":"Ability","fields":{"id":id,"source":"Lindwurm"}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    let entries: Vec<_> = rows.iter().filter(|row| row.action == "phase").collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].detail, phase);
    let expected: Vec<_> = model
        .phases
        .iter()
        .find(|entry| entry.id == phase)
        .unwrap()
        .events
        .iter()
        .filter(|event| event.at == (checkpoint * 10.0).round() as i64)
        .map(|event| (&event.id, 10000))
        .collect();
    assert!(!expected.is_empty());
    let events: Vec<_> = rows
        .iter()
        .filter_map(|row| row.event.as_ref().map(|event| (&event.id, row.at_ms)))
        .collect();
    assert_eq!(events, expected);
    let forecast = super::forecast(&model, &input, 10001, 30000, 100).unwrap();
    assert!(!forecast.events.is_empty());
    assert!(forecast.events.iter().all(|event| event.at_ms > 10001));
}

#[test]
fn r12s_known_pull_transition_and_checkpoint_restart() {
    let model = parse(include_str!("../../timelines/r12s.kdl")).unwrap();
    let input = recording(
        10000,
        serde_json::json!([
            {"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1","isGameEntering":"1"}},
            {"at_ms":1000,"type":"Ability","fields":{"id":"B4D3","source":"Lindwurm"}},
            {"at_ms":1500,"type":"Ability","fields":{"id":"B539","source":"Lindwurm"}},
            {"at_ms":2000,"type":"Ability","fields":{"id":"B51F","source":"Lindwurm"}},
            {"at_ms":3000,"type":"ActorControl","fields":{"command":"4000000F"}},
            {"at_ms":4000,"type":"Ability","fields":{"id":"B528","source":"Lindwurm"}},
            {"at_ms":5000,"type":"InCombat","fields":{"inGameCombat":"1","isGameEntering":"1"}},
            {"at_ms":6000,"type":"Ability","fields":{"id":"B51F","source":"Lindwurm"}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    let phases: Vec<_> = rows
        .iter()
        .filter(|row| row.action == "phase")
        .map(|row| (row.detail.as_str(), row.at_ms))
        .collect();
    assert_eq!(
        phases,
        [
            ("pull/opening", 0),
            ("pre-checkpoint/main", 1000),
            ("pull/checkpoint", 1500),
            ("post-checkpoint/main", 2000),
            ("pull/opening", 5000),
            ("post-checkpoint/main", 6000)
        ]
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.action == "event" && (3000..6000).contains(&row.at_ms))
    );
}

#[rstest]
#[case(false, "BEBD", "pre-checkpoint/main", 70000)]
#[case(true, "B505", "post-checkpoint/main", 200000)]
fn r12s_recovers_when_waiting_autos_are_missing(
    #[case] checkpoint: bool,
    #[case] anchor: &str,
    #[case] phase: &str,
    #[case] at: i64,
) {
    let model = parse(include_str!("../../timelines/r12s.kdl")).unwrap();
    let mut observations = vec![serde_json::json!(
        {"at_ms":0,"type":"InCombat","fields":{"inGameCombat":"1","isGameEntering":"1"}}
    )];
    if checkpoint {
        observations.extend([
            serde_json::json!({"at_ms":1000,"type":"Ability","fields":{"id":"B4D3","source":"Lindwurm"}}),
            serde_json::json!({"at_ms":10000,"type":"Ability","fields":{"id":"B539","source":"Lindwurm"}}),
        ]);
    }
    observations.push(serde_json::json!(
        {"at_ms":at,"type":"Ability","fields":{"id":anchor,"source":"Lindwurm"}}
    ));
    let input = recording(at, serde_json::json!(observations)).unwrap();
    let rows = replay(&model, &input).unwrap();
    assert!(
        rows.iter()
            .any(|row| row.action == "phase" && row.detail == phase && row.at_ms == at)
    );
    assert!(
        rows.iter()
            .any(|row| row.action == "event" && row.at_ms == at)
    );
    let forecast = super::forecast(&model, &input, at, 30000, 100).unwrap();
    assert!(!forecast.events.is_empty());
}

#[rstest]
#[case("zone-exit")]
#[case("wipe")]
#[case("finish")]
#[case("boss-complete")]
fn lifecycle_precedes_cold_recovery(#[case] role: &str) {
    let model = parse(&format!(
        r#"timeline 2 "Priority" {{
        {role} Anchor
        phase fight {{ step main {{
            enter Anchor resume-at=0
            event hit 0 name=Hit
        }} }}
    }}"#
    ))
    .unwrap();
    let input = recording(
        1000,
        serde_json::json!([
            {"at_ms":1000,"type":"Anchor","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row.action.as_str(), "start" | "phase" | "event"))
    );
}

#[test]
fn ambiguous_cold_recovery_requires_reset() {
    let model = parse(
        r#"timeline 2 "Ambiguity" {
        wipe Wipe
        phase fight {
            step first { enter Anchor resume-at=0; event hit 0 name=First; }
            step second {
                enter Anchor resume-at=1
                enter Unique resume-at=1
                event hit 1 name=Second
            }
        }
    }"#,
    )
    .unwrap();
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":1000,"type":"Anchor","fields":{}},
            {"at_ms":2000,"type":"Unique","fields":{}},
            {"at_ms":3000,"type":"Wipe","fields":{}},
            {"at_ms":4000,"type":"Unique","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    assert!(rows.iter().any(|row| row.action == "ambiguous"));
    let events: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            row.event
                .as_ref()
                .map(|event| (event.id.as_str(), row.at_ms))
        })
        .collect();
    assert_eq!(events, [("fight/second/hit", 4000)]);
}

#[test]
fn v2_root_sync_and_utf16_source_identity() {
    let source = "timeline 2 \"한글😀\" { start Pull; sync 1 Anchor; event hit 3 name=Hit; }";
    let model = super::parse_with_file(source, "root.kdl").unwrap();
    let definition = model.catalog.events.values().next().unwrap();
    assert_eq!(definition.source_span.file, "root.kdl");
    assert_eq!(
        definition.source_span.start,
        source
            .get(..source.find("event hit").unwrap())
            .unwrap()
            .encode_utf16()
            .count()
    );
    let input = recording(
        4000,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}},
            {"at_ms":1500,"type":"Anchor","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    assert_eq!(
        rows.iter().find(|row| row.action == "event").unwrap().at_ms,
        3500
    );
}

#[test]
fn v2_direct_loop_sync_projects_all_iterations_without_changing_replay() {
    let model = parse(
        r#"
timeline 2 "Loop" {
    start Pull
    phase fight {
        step main {
            enter Pull
            loop hits start=0 every=2 count=3 {
                sync 0.5 Anchor
                event hit 1.5 name=Hit
            }
        }
    }
}
"#,
    )
    .unwrap();
    let input = recording(
        7000,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}},
            {"at_ms":1000,"type":"Anchor","fields":{}}
        ]),
    )
    .unwrap();
    let before = serde_json::to_value(replay(&model, &input).unwrap()).unwrap();
    let projected = super::forecast(&model, &input, 1000, 5500, 10).unwrap();
    assert_eq!(
        projected
            .events
            .iter()
            .map(|event| event.at_ms)
            .collect::<Vec<_>>(),
        [2000, 4000, 6000]
    );
    assert!(!projected.truncated);
    assert!(
        super::forecast(&model, &input, 1000, 5500, 2)
            .unwrap()
            .truncated
    );
    assert_eq!(
        before,
        serde_json::to_value(replay(&model, &input).unwrap()).unwrap()
    );
    parse(r#"timeline 2 "Bad" { phase fight { step main { enter Pull; loop hits start=0 every=2 { sync 2 Anchor; }; }; }; }"#).unwrap_err();
}

#[test]
fn event_sync_time_defaults_before_section_offset_and_keeps_explicit_overrides() {
    let model = parse(
        r###"
timeline 2 "Defaults" {
    phase main {
        step main {
            enter "Pull"
            section "group" at=5 {
                event hit 2 name="Hit" {
                    sync "Ability" id="A"
                    sync 3 "Other"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
    let times: Vec<_> = model.phases[0].events[0]
        .syncs
        .iter()
        .map(|sync| sync.at)
        .collect();
    assert_eq!(times, [Some(70), Some(80)]);
}

#[rstest]
#[case("waiting-recovery", vec![("fight/first/checkpoint",1000),("fight/last/checkpoint",3000),("fight/last/future",4000)])]
#[case("cold-recovery", vec![("fight/ending/checkpoint",1000),("fight/ending/future",3000),("fight/ending/checkpoint",5000),("fight/ending/future",7000)])]
#[case("direct-syncs", vec![("fight/main/pick/left/hit",2800),("fight/main/pick/merge/hit",6200)])]
#[case("flow-sequence", vec![("fight/opening/choice/left/left-hit",6200),("fight/continuation/joined",9500),("recovery/main/recovered",11000)])]
#[case("branch-join", vec![("fight/opening/choice/left/left-hit",6200),("fight/continuation/joined",9500),("recovery/main/recovered",11000)])]
#[case("shared-body", vec![("fight/opening/choice/shared/common",5700),("fight/opening/choice/left/left-hit",6200),("fight/continuation/joined",9500),("recovery/main/recovered",11000)])]
#[case("shared-scheduled-sequence", vec![("main/main/scheduled-hit",7500),("main/main/choice/left/left-hit",9000),("main/main/loop/pulses/pulse",10500),("main/main/loop/pulses/pulse",12500),("main/main/parent-hit",14500)])]
fn expanded_authoring_contract(#[case] name: &str, #[case] expected: Vec<(&str, i64)>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/timeline");
    let source = std::fs::read_to_string(path.join(format!("{name}.kdl"))).unwrap();
    let input: Recording =
        serde_json::from_slice(&std::fs::read(path.join(format!("{name}.json"))).unwrap()).unwrap();
    for text in [source.clone(), super::expanded(&source).unwrap()] {
        let rows = replay(&parse(&text).unwrap(), &input).unwrap();
        let events: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                row.event
                    .as_ref()
                    .map(|event| (event.id.as_str(), row.at_ms))
            })
            .collect();
        assert_eq!(events, expected);
    }
}

#[test]
fn shared_phase_contract_matches_rangbot() {
    let model = parse(include_str!(
        "../../tests/fixtures/timeline/shared-phase-control.kdl"
    ))
    .unwrap();
    let input: Recording = serde_json::from_str(include_str!(
        "../../tests/fixtures/timeline/shared-phase-control.json"
    ))
    .unwrap();
    let output = replay(&model, &input).unwrap();
    let events: Vec<_> = output
        .iter()
        .filter_map(|row| {
            row.event
                .as_ref()
                .map(|event| (event.id.as_str(), row.at_ms))
        })
        .collect();
    assert_eq!(
        events,
        [
            ("first/main/first-hit", 2000),
            ("second/main/second-zero", 5000),
            ("root", 7000),
            ("second/main/second-hit", 8500)
        ]
    );
    assert!(
        !output
            .iter()
            .any(|row| ["ambiguous", "violation", "unresolved"].contains(&row.action.as_str()))
    );
}

#[rstest]
#[case("from", "Empty from list")]
#[case("from \"first/main\" \"first/main\"", "Duplicate")]
#[case("from \"missing/main\"", "Unknown")]
#[case(
    "from \"first/main\"; from \"second/main\"",
    "one from property or list"
)]
fn entry_lists_reject_invalid_sources(#[case] list: &str, #[case] message: &str) {
    let source = format!(
        r###"timeline 2 "Invalid" {{
	phase first {{
        step main {{ enter "Start";		}}
	}}
	phase second {{        step main {{ enter "Next" {{ {list};			}};		}}
	}}
}}"###
    );
    assert!(format!("{:#}", parse(&source).unwrap_err()).contains(message));
}

#[test]
fn direct_phase_sync_precedes_event_sync_by_window_end_then_source_order() {
    let model = parse(
        r###"
timeline 2 "Sync" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            sync 5 "Anchor" before=3 after=2
            event hit 10 name="Hit" {
                sync 4 "Anchor" before=2 after=3
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        11000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":5000,"type":"Anchor","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert_eq!(
        output.iter().find(|row| row.event.is_some()).unwrap().at_ms,
        10000
    );
    assert_eq!(model.event_count, 1);
}

fn recording(end_ms: i64, observations: serde_json::Value) -> Result<Recording> {
    Ok(serde_json::from_value(
        serde_json::json!({"version":1,"end_ms":end_ms,"observations":observations}),
    )?)
}

#[rstest]
#[case(4000, vec![0, 2000], true)]
#[case(6000, vec![0, 2000, 4000], false)]
#[case(8000, vec![0, 2000, 4000], false)]
fn counted_loop_finishes_without_running_signal_exit(
    #[case] stop_at: i64,
    #[case] expected: Vec<i64>,
    #[case] signal_exit: bool,
) {
    let model = parse(
        r###"
timeline 2 "Count" {
    start "Pull"
    phase boss {
        step main {
            enter "Pull"
            loop rotation start=0.0 every=2.0 count=3 {
                until "Stop" at=0.0
                event pulse 0.0 name="Pulse"
                event tail 1.5 name="Tail"
            }
            loop next start=6.0 every=2.0 count=1 {
                event next-event 0.0 name="Next"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        11000,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}}, {"at_ms":stop_at,"type":"Stop","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    let displays = |id| {
        rows.iter()
            .filter(|row| row.event.as_ref().is_some_and(|e| e.id == id))
            .map(|row| row.at_ms)
            .collect::<Vec<_>>()
    };
    assert_eq!(displays("boss/main/loop/rotation/pulse"), expected);
    assert_eq!(
        displays("boss/main/loop/rotation/tail").last().copied(),
        Some(if signal_exit { 3500 } else { 5500 })
    );
    assert_eq!(rows.iter().any(|row| row.action == "until"), signal_exit);
    assert_eq!(displays("boss/main/loop/next/next-event"), [6000]);
    assert!(!rows.iter().any(|row| row.action == "violation"));
}

#[test]
fn counted_loop_is_exhausted_by_forward_sync_past_its_cap() {
    let model = parse(
        r###"
timeline 2 "Jump" {
    start "Pull"
    phase boss {
        step main {
            enter "Pull"
            event advance 20.0 name="Advance" {
                sync 20.0 "Jump" before=30.0 after=0.1
            }
            loop rotation start=5.0 every=2.0 count=2 {
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        12000,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}}, {"at_ms":1000,"type":"Jump","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.action == "skipped" && row.detail == "boss/main/loop/rotation/pulse")
            .count(),
        2
    );
    assert!(!rows.iter().any(|row| {
        row.event
            .as_ref()
            .is_some_and(|event| event.id == "boss/main/loop/rotation/pulse")
    }));
}

#[rstest]
#[case(false)]
#[case(true)]
fn option_resume_skips_past_displays_without_shifting_parent(#[case] in_loop: bool) {
    let branch = r#"branch "route" open=0.0 close=4.0 {
        option "a" {
            when "Select"
            when "Impact" resume-at=0.8
            event "first" 0.8 name="First" { sync 0.8 "OldSync"; }
            event "second" 5.3 name="Second"
        }
        option "b" { when "Other"; when "OtherImpact" resume-at=0.8; }
    }"#;
    let body = if in_loop {
        format!(r#"loop "round" start=0.0 every=10.0 {{ {branch} }}"#)
    } else {
        branch.to_owned()
    };
    let model = parse(&format!(
        r###"timeline 2 "Resume" {{
            start "Pull"
	phase boss {{            step main {{ enter "Pull"; event parent 6.0 name="Parent"; {body} 		}}
	}}
}}"###
    ))
    .unwrap();
    let input = recording(
        9500,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}},
            {"at_ms":2800,"type":"Impact","fields":{}},
            {"at_ms":3100,"type":"OldSync","fields":{}},
            {"at_ms":3200,"type":"OtherImpact","fields":{}},
            {"at_ms":3300,"type":"Impact","fields":{}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    let selection = rows.iter().find(|row| row.action == "option").unwrap();
    assert_eq!(selection.resume_at_ms, Some(800));
    assert_eq!(rows.iter().filter(|row| row.action == "option").count(), 1);
    assert!(
        !rows
            .iter()
            .any(|row| matches!(row.action.as_str(), "ambiguous" | "sync"))
    );
    let displays = rows
        .iter()
        .filter_map(|row| Some((row.event.as_ref()?.id.as_str(), row.at_ms)))
        .collect::<Vec<_>>();
    let second = if in_loop {
        "boss/main/loop/round/route/a/second"
    } else {
        "boss/main/route/a/second"
    };
    assert_eq!(displays, [("boss/main/parent", 6000), (second, 7300)]);
}

#[test]
fn competing_offsets_for_one_option_are_rejected_or_reported() {
    let timeline = r###"
timeline 2 "Conflict" {
    start "Pull"
    phase boss {
        step main {
            enter "Pull"
            branch route open=0.0 close=4.0 {
                option a {
                    when "Signal"
                    when "Signal" resume-at=0.8
                }
                option b {
                    when "Other"
                }
            }
        }
    }
}
"###;
    let error = parse(timeline).unwrap_err();
    assert!(format!("{error:#}").contains("Conflicting entry offsets for option a"));
    let outside = timeline.replace(
        r#"when "Signal" resume-at=0.8"#,
        r#"when "Signal" resume-at=0.8 at=4.0 before=0.0 after=1.0;"#,
    );
    parse(&outside).unwrap();
    // A regex intersection cannot be proved by the literal overlap check.
    let runtime = timeline.replace(
        r#"when "Signal" resume-at=0.8"#,
        r#"when "Signal" resume-at=0.8 { regex "id" "^A$"; }"#,
    );
    let model = parse(&runtime).unwrap();
    let input = recording(
        2000,
        serde_json::json!([
            {"at_ms":0,"type":"Pull","fields":{}}, {"at_ms":1000,"type":"Signal","fields":{"id":"A"}}
        ]),
    )
    .unwrap();
    let rows = replay(&model, &input).unwrap();
    assert!(rows.iter().any(|row| row.action == "ambiguous"));
    assert!(!rows.iter().any(|row| row.action == "option"));
}

#[rstest]
#[case(false, 5000)]
#[case(true, 3000)]
fn scoped_completion_and_checkpoint_restart(#[case] same_packet: bool, #[case] post_at: i64) {
    let transition = if same_packet {
        r#"enter "Defeat" from="pre/main";"#
    } else {
        ""
    };
    let model = parse(&format!(
        r###"timeline 2 "Checkpoint" {{
            start "Pull"
            start "Checkpoint"
            wipe "Wipe"
            boss-complete "Defeat" from="pre/main" {{	}}
	phase pre {{            step main {{
                enter "Pull"
                event cancelled 3.0 name="Cancelled"
		}}
	}}
	phase post {{            step main {{
                enter "Checkpoint"
                {transition}
                event first 1.0 name="First"
                event later 5.0 name="Later"
		}}
	}}
}}"###
    ))
    .unwrap();
    let observations = [
        (0, "Defeat"),
        (1000, "Pull"),
        (3000, "Defeat"),
        (4000, "Defeat"),
        (5000, "Checkpoint"),
        (5100, "Checkpoint"),
        (5500, "Defeat"),
        (6500, "Wipe"),
        (9000, "Checkpoint"),
        (9100, "Checkpoint"),
        (9500, "Defeat"),
    ];
    let input =
        recording(
            10000,
            serde_json::json!(observations.map(|(at_ms, kind)| {
                serde_json::json!({"at_ms":at_ms,"type":kind,"fields":{}})
            })),
        )
        .unwrap();
    let rows = replay(&model, &input).unwrap();
    let lifecycle = rows
        .iter()
        .filter(|row| {
            matches!(
                row.action.as_str(),
                "start" | "phase" | "boss-complete" | "wipe"
            )
        })
        .map(|row| (row.action.as_str(), row.detail.as_str(), row.at_ms, row.run))
        .collect::<Vec<_>>();
    assert_eq!(
        lifecycle,
        [
            ("start", "", 1000, 1),
            ("phase", "pre/main", 1000, 1),
            ("boss-complete", "", 3000, 1),
            ("phase", "post/main", post_at, 1),
            ("wipe", "", 6500, 1),
            ("start", "", 9000, 2),
            ("phase", "post/main", 9000, 2),
        ]
    );
    let displayed = rows
        .iter()
        .filter_map(|row| {
            row.event
                .as_ref()
                .map(|event| (event.id.as_str(), row.at_ms, row.run))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        displayed,
        [
            ("post/main/first", post_at + 1000, 1),
            ("post/main/first", 10000, 2)
        ]
    );
}

#[test]
fn scoped_completion_requires_a_declared_phase() {
    let error =
        parse(r###"timeline 2 "Invalid" { boss-complete "Defeat" from="missing/main" {	};}"###)
            .unwrap_err();
    assert!(error.to_string().contains("Unknown phase in from: missing"));
}

#[test]
fn occurrence_context_survives_sync_and_distinguishes_phase_reentry() {
    let model = parse(
        r###"
timeline 2 "Identity" {
    start "Begin"
    phase a {
        step main {
            enter "Begin"
            loop rotation start=0.0 every=4.0 {
                event pulse 0.0 name="Pulse" {
                    sync 0.0 "Pulse"
                }
            }
        }
    }
    phase b {
        step main {
            enter "Other"
            event other 0.0 name="Other"
        }
    }
}
"###,
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
        .filter(|row| {
            row.event
                .as_ref()
                .is_some_and(|event| event.id == "a/main/loop/rotation/pulse")
        })
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
    let future = super::forecast(&model, &input, 10000, 5000, 10000).unwrap();
    assert!(
        future
            .events
            .iter()
            .any(|event| event.id == "a/main/loop/rotation/pulse"
                && event.context.phase_instance == 3
                && event.context.iteration == Some(2))
    );
}

#[test]
fn phase_jump_preserves_modeled_iteration_and_exit_retains_running_cycle() {
    let model = parse(
        r###"
timeline 2 "Jump" {
    start "Begin"
    phase a {
        step main {
            enter "Begin"
            event advance 20.0 name="Advance" visible=#false {
                sync 20.0 "Advance" before=25.0 after=25.0
            }
            loop rotation start=10.0 every=4.0 {
                until "End" at=0.0 before=0.0
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Begin","fields":{}},{"at_ms":1000,"type":"Advance","fields":{}},{"at_ms":4000,"type":"End","fields":{}}
        ]),
    ).unwrap();
    let rows = replay(&model, &input).unwrap();
    let pulse = rows
        .iter()
        .find(|row| {
            row.event
                .as_ref()
                .is_some_and(|event| event.id == "a/main/loop/rotation/pulse")
        })
        .expect("jumped loop emits");
    assert_eq!(pulse.at_ms, 3000);
    assert_eq!(
        pulse.context.as_ref().expect("event context").iteration,
        Some(4)
    );
    for row in rows.iter().filter(|row| row.action == "until") {
        assert_eq!(
            row.context.as_ref().expect("exit context").iteration,
            Some(4)
        );
    }
    let future = super::forecast(&model, &input, 1000, 7000, 10000).unwrap();
    assert_eq!(
        future
            .events
            .iter()
            .filter(|event| event.id == "a/main/loop/rotation/pulse")
            .map(|event| event.context.iteration)
            .collect::<Vec<_>>(),
        vec![Some(4), Some(5)]
    );
}

#[rstest]
#[case::version("timeline 1 \"x\" {}")]
#[case::precision(r###"timeline 2 "x" { event "e" 0.01 name="x";}"###)]
#[case::boolean(r###"timeline 2 "x" { event "e" 1.0 name="x" visible="false";}"###)]
#[case::unknown_property(r###"timeline 2 "x" { event "e" 1.0 name="x" typo=#true;}"###)]
#[case::duplicate_property(r###"timeline 2 "x" { event "e" 1.0 name="x" name="y";}"###)]
#[case::duplicate_id(r###"timeline 2 "x" { event "e" 1.0 name="x"; event "e" 2.0 name="y";}"###)]
#[case::unknown_phase(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "X" from="missing/main"
        }
    }
}
"###
)]
#[case::missing_from(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "X" at=1.0
        }
    }
}
"###
)]
#[case::partial_window(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "X"
            event e 1.0 name="x" {
                sync 1.0 "A" before=1.0
            }
        }
    }
}
"###
)]
#[case::duplicate_field(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "X" id="A" {
                field "id" "B"
            }
        }
    }
}
"###
)]
#[case::invalid_regex(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "X" {
                regex "id" "["
            }
        }
    }
}
"###
)]
#[case::unknown_node(r###"timeline 2 "x" { unknown;}"###)]
#[case::removed_loop_clock(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "Start"
            loop l clock="phase" start=1 every=2 count=3 {
                event hit 0 name="Hit"
            }
        }
    }
}
"###
)]
#[case::removed_scheduled_loop_clock(
    r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "Start"
            section "scheduled" at=2 {
                loop l clock="phase" start=1 every=2 count=3 {
                    event hit 0 name="Hit"
                }
            }
        }
    }
}
"###
)]
#[case::removed_flow_loop_clock(r###"timeline 2 "x" { phase "fight" { step p { enter "Start"; loop l clock="phase" start=1 every=2 count=3 { event hit 0 name="Hit";			};		};	};}"###)]
fn structure_rejects_invalid_documents(#[case] source: &str) {
    assert!(parse(source).is_err(), "Accepted {source}");
}

#[test]
fn branch_constraints_and_replay_capability_are_separate() {
    let body = r###"
timeline 2 "Branches" {
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=20.0 {
                branch choice open=0.0 close=5.0 {
                    option left {
                        when "A"
                        event left 3.0 name="Left"
                    }
                    option right {
                        when "B"
                        event right 3.0 name="Right"
                    }
                    merge {
                        when "C" at=10.0 before=1.0 after=1.0
                        event shared 2.0 name="Shared"
                    }
                }
            }
        }
    }
}
"###;
    let model = parse(body).unwrap();
    assert_eq!(model.event_count, 3);
    let rotation = model.phases.first().unwrap().loops.first().unwrap();
    assert_eq!(rotation.id, "p/main/loop/rotation");
    assert_eq!(rotation.branches.first().unwrap().options.len(), 2);
    let input = recording(0, serde_json::json!([])).unwrap();
    replay(&model, &input).unwrap();
    for invalid in [
        body.replace("when \"C\" at=10.0 before=1.0 after=1.0", "when \"C\""),
        body.replace("at=10.0", "at=17.0"),
        body.replace(
            "event shared 2.0 name=\"Shared\"",
            "loop \"nested\" start=0.0 every=1.0 {}",
        ),
        body.replace(
            "event left 3.0 name=\"Left\"",
            "loop \"nested\" start=0.0 every=1.0 {}",
        ),
        body.replace("when \"A\"", "when \"A\"; requires \"choice\" \"right\";"),
        body.replace("when \"A\"", "when \"A\"; requires \"absent\" \"right\";"),
    ] {
        assert!(parse(&invalid).is_err(), "Accepted {invalid}");
    }
}

#[test]
fn loop_branch_selects_and_merges_independently_each_iteration() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                        event left-hit 1.0 name="Left"
                    }
                    option right {
                        when "B"
                        event right-hit 1.0 name="Right"
                    }
                    merge {
                        when "Join" at=4.0 before=1.0 after=1.0
                        event shared 1.0 name="Shared"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/left"
        && r.at_ms == 1000));
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 10000));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/left-hit:")
            && r.at_ms == 2000
    }));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/right/right-hit:")
            && r.at_ms == 11000
    }));
    let shared: Vec<_> = output
        .iter()
        .filter(|r| {
            r.action == "event"
                && r.detail
                    .starts_with("p/main/loop/rotation/choice/merge/shared:")
        })
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(shared, [5000, 15000]);
    assert!(!output.iter().any(|r| r.action == "unresolved"));
}

#[test]
fn branch_inside_option_loop_is_cancelled_on_outer_merge() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch outer open=0.0 close=3.0 {
                option left {
                    when "A"
                    loop rotation start=0.0 every=10.0 {
                        branch inner open=0.0 close=2.0 {
                            option one {
                                when "C"
                                event hit 0.0 name="Hit"
                            }
                            option two {
                                when "D"
                            }
                        }
                    }
                }
                option right {
                    when "B"
                }
                merge {
                    when "Merge"
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/outer/left/loop/rotation/inner/one"
        && r.at_ms == 1000));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/outer/left/loop/rotation/inner/one/hit:")
            && r.at_ms == 1000
    }));
    assert!(
        output
            .iter()
            .any(|r| r.action == "merge" && r.detail == "p/main/outer")
    );
    assert!(
        !output.iter().any(
            |r| r.action == "option" && r.detail == "p/main/outer/left/loop/rotation/inner/two"
        )
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn phase_sync_can_start_a_nested_branch_in_an_option_loop() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 5.0 name="Anchor" {
                sync 5.0 "Sync" before=5.0 after=0.1
            }
            branch outer open=0.0 close=2.0 {
                option left {
                    when "A"
                    loop rotation start=2.0 every=10.0 {
                        branch inner open=0.0 close=3.0 {
                            option one {
                                when "C"
                                event hit 0.0 name="Hit"
                            }
                            option two {
                                when "D"
                            }
                        }
                    }
                }
                option right {
                    when "B"
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/outer/left/loop/rotation/inner/one"
        && r.at_ms == 1500));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/outer/left/loop/rotation/inner/one/hit:")
            && r.at_ms == 1500
    }));
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
        r###"timeline 2 "x" {{ start "Start";	phase p {{ step main {{ enter "Start"; {body} 		}} 	}}
}}"###
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
    let _ = running;
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.at_ms >= 1000)
    );
}

#[test]
fn another_loop_can_exit_at_nested_loop_start_before_selection() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                until "End" at=2.0 before=0.0
            }
            loop second start=2.0 every=10.0 {
                branch inner open=0.0 close=2.0 {
                    option one {
                        when "C"
                    }
                    option two {
                        when "D"
                    }
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "until" && r.detail == "p/main/loop/first" && r.at_ms == 2000)
    );
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/second/inner/one"
        && r.at_ms == 2000));
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn another_loop_exit_does_not_restore_old_nested_iteration_history() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            loop rotation start=0.0 every=2.0 {
                branch choice open=0.0 close=1.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
            loop waiting start=5.0 every=1.0 {
                until "Skip" at=2.0 before=0.0
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch later open=0.0 close=3.0 {
                option left {
                    when "C"
                    requires "first/main/loop/rotation/choice" "left"
                }
                option right {
                    when "D"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "until" && r.detail == "first/main/loop/waiting")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "second/main/later/left")
    );
}

#[test]
fn outer_merge_at_option_loop_start_preserves_previous_nested_choice() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            enter "Return"
            branch outer open=0.0 close=3.0 {
                option left {
                    when "A"
                    loop rotation start=1.0 every=10.0 {
                        branch inner open=0.0 close=1.0 {
                            option one {
                                when "C"
                            }
                            option two {
                                when "D"
                            }
                        }
                    }
                }
                option right {
                    when "B"
                }
                merge {
                    when "Merge"
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch later open=0.0 close=2.0 {
                option one {
                    when "Check"
                    requires "first/main/outer/left/loop/rotation/inner" "one"
                }
                option two {
                    when "Other"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "merge" && r.detail == "first/main/outer" && r.at_ms == 4500)
    );
    assert!(
        output.iter().any(|r| r.action == "option"
            && r.detail == "second/main/later/one"
            && r.at_ms == 5500)
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
}

#[test]
fn nested_option_sync_corrects_iteration_and_future_iterations() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 10.0 name="Phase"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                        event left-hit 2.0 name="Left" {
                            sync 2.0 "Sync" before=2.0 after=2.0
                        }
                    }
                    option right {
                        when "B"
                        event right-hit 1.0 name="Right"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| r.action == "sync"
        && r.detail.starts_with("p/main/loop/rotation")
        && r.at_ms == 2000));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/left-hit:")
            && r.at_ms == 2000
    }));
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 9000));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/right/right-hit:")
            && r.at_ms == 10000
    }));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 10000));
}

#[test]
fn phase_sync_does_not_move_an_active_loop_branch() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 6.0 name="Phase" {
                sync 5.0 "Sync" before=5.0 after=5.0
            }
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=4.0 {
                    option left {
                        when "A"
                        event left-hit 2.0 name="Left"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 2500));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/left-hit:")
            && r.at_ms == 3000
    }));
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 10000));
}

#[test]
fn phase_and_loop_branches_report_competing_choices() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch outer open=0.0 close=3.0 {
                option one {
                    when "A"
                }
                option two {
                    when "P"
                }
            }
            loop rotation start=0.0 every=10.0 {
                branch inner open=0.0 close=3.0 {
                    option one {
                        when "A"
                    }
                    option two {
                        when "N"
                    }
                }
            }
        }
    }
}
"###,
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
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "p/main/loop/rotation/inner/two")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "p/main/outer/two")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail.ends_with("/one"))
    );
}

#[test]
fn phase_option_sync_does_not_shift_an_active_nested_branch() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch outer open=0.0 close=3.0 {
                option one {
                    when "O"
                    event outer-hit 4.0 name="Outer" {
                        sync 4.0 "Sync" before=4.0 after=4.0
                    }
                }
                option two {
                    when "P"
                }
            }
            loop rotation start=0.0 every=10.0 {
                branch inner open=2.0 close=4.0 {
                    option one {
                        when "N"
                    }
                    option two {
                        when "M"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/outer/one/outer-hit:")
        && r.at_ms == 2000));
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/inner/one"
        && r.at_ms == 2500));
}

#[test]
fn phase_sync_at_nested_loop_start_runs_before_the_start_timer() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 0.0 name="Anchor" {
                sync 0.0 "Sync" before=0.0 after=3.0
            }
            loop rotation start=2.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "option" && r.detail == "p/main/loop/rotation/choice/left")
    );
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 4000));
}

#[test]
fn phase_option_sync_at_nested_loop_start_runs_before_the_start_timer() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch outer open=0.0 close=2.0 {
                option one {
                    when "O"
                    event anchor 0.0 name="Anchor" {
                        sync 0.0 "Sync" before=0.0 after=3.0
                    }
                }
                option two {
                    when "P"
                }
            }
            loop rotation start=2.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
            .any(|r| r.action == "option" && r.detail == "p/main/loop/rotation/choice/left")
    );
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 3500));
}

#[test]
fn phase_sync_skips_nested_iterations_without_guessing_choices() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            event anchor 25.0 name="Anchor" {
                sync 25.0 "Sync" before=25.0 after=0.1
            }
            loop rotation start=3.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                        event left-hit 0.0 name="Left"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch later open=0.0 close=3.0 {
                option left {
                    when "C"
                    requires "first/main/loop/rotation/choice" "left"
                }
                option right {
                    when "D"
                }
            }
        }
    }
}
"###,
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
            .filter(|r| r.action == "unresolved"
                && r.detail == "first/main/loop/rotation/choice"
                && r.at_ms == 1000)
            .count(),
        2
    );
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "first/main/loop/rotation/choice/left"
        && r.at_ms == 1500));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("first/main/loop/rotation/choice/left/left-hit:")
            && r.at_ms == 1500
    }));
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "second/main/later/left")
    );
}

#[test]
fn phase_sync_past_the_current_nested_window_marks_it_unresolved() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 8.0 name="Anchor" {
                sync 8.0 "Sync" before=8.0 after=0.1
            }
            loop rotation start=3.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "unresolved"
        && r.detail == "p/main/loop/rotation/choice"
        && r.at_ms == 1000));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "p/main/loop/rotation/choice/left")
    );
}

#[test]
fn separate_signal_at_phase_sync_timestamp_can_select_new_loop_branch() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 3.0 name="Anchor" {
                sync 3.0 "Sync" before=3.0 after=0.1
            }
            loop rotation start=3.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                        event hit 0.0 name="Hit"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/left"
        && r.at_ms == 1000));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/hit:")
            && r.at_ms == 1000
    }));
}

#[test]
fn nested_sync_skips_crossed_option_events() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=3.0 {
                    option left {
                        when "A"
                        event early 1.0 name="Early"
                        event target 4.0 name="Target" {
                            sync 4.0 "Sync" before=4.0 after=4.0
                        }
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "skipped"
        && r.detail == "p/main/loop/rotation/choice/left/early"
        && r.at_ms == 1500));
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/target:")
            && r.at_ms == 1500
    }));
    assert!(!output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/left/early:")
    }));
}

#[test]
fn nested_merge_sync_corrects_its_iteration_without_moving_the_phase() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 10.0 name="Phase"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                    merge {
                        when "Join" at=3.0 before=1.0 after=1.0
                        event shared 1.0 name="Shared" {
                            sync 1.0 "Sync" before=1.0 after=1.0
                        }
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/loop/rotation/choice/merge/shared:")
            && r.at_ms == 3500
    }));
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/right"
        && r.at_ms == 9500));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 10000));
}

#[test]
fn forward_iteration_sync_reports_crossed_nested_window_at_sync_time() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                event anchor 6.0 name="Anchor" {
                    sync 6.0 "Sync" before=6.0 after=0.1
                }
                branch choice open=4.0 close=5.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "unresolved"
        && r.detail == "p/main/loop/rotation/choice"
        && r.at_ms == 1000));
}

#[test]
fn loop_exit_precedes_nested_option_sync_on_the_same_signal() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                until "Ability" id="X" at=2.0 before=1.0
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                        event hit 2.0 name="Hit" {
                            sync 2.0 "Ability" id="X" before=2.0 after=2.0
                        }
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
            .any(|r| r.action == "until" && r.detail == "p/main/loop/rotation" && r.at_ms == 2000)
    );
    assert!(!output.iter().any(|r| {
        r.action == "sync"
            || (r.action == "event"
                && r.detail
                    .starts_with("p/main/loop/rotation/choice/left/hit:"))
    }));
}

#[test]
fn loop_sync_moves_nested_branch_window_and_does_not_reopen_closed_choices() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                event anchor 3.0 name="Anchor" {
                    sync 3.0 "Forward" before=3.0 after=3.0
                    sync 0.0 "Back" before=0.0 after=9.0
                }
                branch choice open=4.0 close=6.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "p/main/loop/rotation/choice/left"
        && r.at_ms == 3000));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "p/main/loop/rotation/choice/right")
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
        r###"timeline 2 "x" {{ start "Start";	phase p {{ step main {{ enter "Start"; {body} 		}} 	}}
}}"###
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
            .any(|r| r.action == "option" && r.detail.ends_with("/choice/left"))
    );
    assert!(
        output.iter().any(|r| r.action == "option"
            && r.detail.ends_with("/choice/right")
            && r.at_ms == 3000)
    );
}

#[test]
fn missing_new_iteration_choice_does_not_reuse_an_older_selection() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch later open=0.0 close=3.0 {
                option left {
                    when "C"
                    requires "first/main/loop/rotation/choice" "left"
                }
                option right {
                    when "D"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "unresolved" && r.detail == "first/main/loop/rotation/choice")
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail.contains("missing prior choice"))
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" && r.detail == "second/main/later/left")
    );
}

#[test]
fn loop_branch_can_require_a_choice_from_an_earlier_phase() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            branch earlier open=0.0 close=2.0 {
                option left {
                    when "A"
                }
                option right {
                    when "B"
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "C"
                        requires "first/main/earlier" "left"
                    }
                    option right {
                        when "D"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "option"
        && r.detail == "second/main/loop/rotation/choice/left"
        && r.at_ms == 2500));
}

#[test]
fn loop_exit_at_iteration_boundary_preserves_prior_choice_history() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase first {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                until "Ability" id="end" at=10.0 before=0.0
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "Ability" id="left"
                    }
                    option right {
                        when "Ability" id="right"
                    }
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch later open=0.0 close=3.0 {
                option left {
                    when "C"
                    requires "first/main/loop/rotation/choice" "left"
                }
                option right {
                    when "D"
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "until"
        && r.detail == "first/main/loop/rotation"
        && r.at_ms == 10000));
    assert!(
        output
            .iter()
            .any(|r| r.action == "option" && r.detail == "second/main/later/left")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "first/main/loop/rotation/choice")
    );
}

#[test]
fn loop_exit_and_new_iteration_choice_on_same_observation_are_ambiguous() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                until "Ability" id="same" at=10.0 before=0.0
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "Ability" id="same"
                    }
                    option right {
                        when "Ability" id="other"
                    }
                }
            }
        }
    }
}
"###,
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
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                until "Ability" id="same" at=4.0 before=0.0
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "Ability" id="left"
                    }
                    option right {
                        when "Ability" id="right"
                    }
                    merge {
                        when "Ability" id="same" at=4.0 before=1.0 after=1.0
                    }
                }
            }
        }
    }
}
"###,
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
        r###"
timeline 2 "x" {
    start "Start"
    wipe "Wipe"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=10.0 {
                branch choice open=0.0 close=2.0 {
                    option left {
                        when "A"
                        event left-hit 2.0 name="Left"
                    }
                    option right {
                        when "B"
                    }
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "unobserved"
        && r.detail == "p/main/loop/rotation/choice/left/left-hit"
        && r.at_ms == 2000));
}

#[test]
fn option_loop_preserves_exit_and_body() {
    let model = parse(
        r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "Start"
            branch b open=0.0 close=3.0 {
                option a {
                    when "A"
                    loop hold start=1.0 every=5.0 {
                        until "End" at=2.0 before=1.0
                        event pulse 1.0 name="Pulse"
                    }
                }
                option b {
                    when "B"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
    let phase = model.phases.first().unwrap();
    let option = phase.branches.first().unwrap().options.first().unwrap();
    let hold = option.loops.first().unwrap();
    assert_eq!((hold.start, hold.every), (10, 50));
    assert_eq!(hold.untils.len(), 1);
    assert_eq!(
        hold.events.first().unwrap().id,
        "p/main/b/a/loop/hold/pulse"
    );
    replay(&model, &recording(0, serde_json::json!([])).unwrap()).unwrap();
}

#[test]
fn selected_option_loop_uses_phase_anchor_and_merge_cancels_it() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    loop left-loop start=0.0 every=1.0 {
                        until "End" at=2.0 before=0.0
                        event left-pulse 0.0 name="Left"
                    }
                }
                option right {
                    when "Right"
                    loop right-loop start=0.0 every=1.0 {
                        event right-pulse 0.0 name="Right"
                    }
                }
                merge {
                    when "Merge"
                    event shared 0.0 name="Shared"
                }
            }
        }
    }
}
"###,
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
        .filter(|r| {
            r.action == "event"
                && r.detail
                    .starts_with("p/main/choice/left/loop/left-loop/left-pulse:")
        })
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1049, 2049]);
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/choice/merge/shared:")
        && r.at_ms == 3000));
    assert!(!output.iter().any(|r| {
        r.detail
            .starts_with("p/main/choice/right/loop/right-loop/right-pulse")
            || r.action == "until"
    }));
}

#[test]
fn rounded_up_selection_fires_zero_events_at_the_observation() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    event zero 0.0 name="Zero"
                    loop hold start=0.0 every=1.0 {
                        event pulse 0.0 name="Pulse"
                    }
                }
                option right {
                    when "Right"
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/choice/left/zero:")
        && r.at_ms == 1051));
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| {
            r.action == "event" && r.detail.starts_with("p/main/choice/left/loop/hold/pulse:")
        })
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1051, 2051]);
}

#[test]
fn same_timestamp_sync_can_delay_new_option_zero_events() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 0.0 name="Anchor" {
                sync 0.0 "Sync" before=0.0 after=3.0
            }
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    event zero 0.0 name="Zero"
                    loop hold start=0.0 every=1.0 {
                        event pulse 0.0 name="Pulse"
                    }
                }
                option right {
                    when "Right"
                }
            }
        }
    }
}
"###,
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
    for id in [
        "p/main/choice/left/zero:",
        "p/main/choice/left/loop/hold/pulse:",
    ] {
        assert!(
            output
                .iter()
                .any(|r| r.action == "event" && r.detail.starts_with(id) && r.at_ms == 2102)
        );
        assert!(
            !output
                .iter()
                .any(|r| r.action == "event" && r.detail.starts_with(id) && r.at_ms == 1051)
        );
    }
}

#[test]
fn rounded_up_merge_fires_zero_event_at_its_signal() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    loop hold start=2.0 every=1.0 {
                        until "End" at=0.0 before=0.0
                    }
                }
                option right {
                    when "Right"
                }
                merge {
                    when "Merge"
                    event merge-zero 0.0 name="Merge"
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/choice/merge/merge-zero:")
        && r.at_ms == 2051));
}

#[test]
fn option_loop_starts_when_phase_loop_exits_at_the_same_boundary() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
            }
            branch choice open=0.0 close=2.0 {
                option left {
                    when "Left"
                    loop second start=1.0 every=1.0 {
                        event pulse 0.0 name="Pulse"
                    }
                }
                option right {
                    when "Right"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    assert!(output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/choice/left/loop/second/pulse:")
            && r.at_ms == 2000
    }));
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn simultaneous_option_and_phase_loop_starts_follow_document_order() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=2.0 {
                option left {
                    when "Left"
                    loop option-loop start=0.0 every=1.0 {
                        event a 0.0 name="A"
                    }
                }
                option right {
                    when "Right"
                }
            }
            loop phase-loop start=1.0 every=1.0 {
                event b 0.0 name="B"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Left","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(!output.iter().any(|r| {
        r.action == "event"
            && r.detail
                .starts_with("p/main/choice/left/loop/option-loop/a:")
            && r.at_ms == 1000
    }));
    assert!(output.iter().any(|r| r.action == "violation"
        && r.detail.contains("p/main/loop/phase-loop")
        && r.at_ms == 1000));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/loop/phase-loop/b:"))
    );
}

#[test]
fn unselected_option_loop_does_not_block_a_phase_loop() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=2.0 {
                option left {
                    when "Left"
                    loop option-loop start=0.0 every=1.0 {
                        event a 0.0 name="A"
                    }
                }
                option right {
                    when "Right"
                }
            }
            loop phase-loop start=1.0 every=1.0 {
                event b 0.0 name="B"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        1500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":500,"type":"Right","fields":{}}
        ]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/phase-loop/b:")
        && r.at_ms == 1000));
    assert!(!output.iter().any(|r| {
        r.action == "violation"
            || r.detail
                .starts_with("p/main/choice/left/loop/option-loop/a:")
    }));
}

#[test]
fn merge_and_option_loop_exit_on_one_observation_are_ambiguous() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    loop hold start=0.0 every=1.0 {
                        until "X" at=1.0 before=0.0
                    }
                }
                option right {
                    when "Right"
                }
                merge {
                    when "X"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 5.0 name="Anchor" {
                sync 5.0 "Forward" before=5.0 after=5.0
            }
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Left"
                    loop hold start=2.0 every=2.0 {
                        event pulse 0.0 name="Pulse"
                    }
                }
                option right {
                    when "Right"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
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
            .filter(|r| r.action == "skipped" && r.detail == "p/main/choice/left/loop/hold/pulse")
            .count(),
        1
    );
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| {
            r.action == "event" && r.detail.starts_with("p/main/choice/left/loop/hold/pulse:")
        })
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1500, 3500]);
}

#[rstest]
#[case::before_start(1000, vec![])]
#[case::on_start(2000, vec![])]
#[case::after_two_pulses(4500, vec![2000, 3000, 4000])]
fn simple_loop_repeats_and_exits(#[case] until_ms: i64, #[case] pulses: Vec<i64>) {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=2.0 every=1.0 {
                until "End" at=1.0 before=1.0
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/rotation/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(actual, pulses);
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "p/main/loop/rotation")
    );
}

#[test]
fn finite_loop_exit_window_reports_unresolved_but_keeps_repeating() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=0.0 every=1.0 {
                until "End" at=1.0 before=0.0 after=0.1
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "unresolved" && r.detail == "p/main/loop/rotation")
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/rotation/pulse:"))
            .count(),
        3
    );
}

#[test]
fn excessive_loop_iterations_are_rejected() {
    let model = parse(
        r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "Start"
            loop l start=0.0 every=0.1
        }
    }
}
"###,
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
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
                event a 0.0 name="A"
            }
            loop second start=2.0 every=1.0 {
                until "EndSecond" at=5.0 before=0.0
                event b 0.0 name="B"
            }
        }
    }
}
"###,
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
        matches!(events.as_slice(), [(0, a), (1000, b), (2000, c), (3000, d), (4000, e)] if a.starts_with("p/main/loop/first/a:") && b.starts_with("p/main/loop/first/a:") && c.starts_with("p/main/loop/second/b:") && d.starts_with("p/main/loop/second/b:") && e.starts_with("p/main/loop/second/b:"))
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn simultaneous_active_loops_report_violation_and_do_not_restart() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop second start=1.0 every=1.0 {
                event b 0.0 name="B"
            }
            loop first start=0.0 every=1.0 {
                until "EndFirst" at=3.0 before=0.0
                event a 0.0 name="A"
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "violation"
        && r.detail.contains("p/main/loop/second")
        && r.at_ms == 1000));
    assert!(output.iter().all(|row| row.at_ms <= 1000));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/loop/second/b:"))
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/first/a:"))
            .count(),
        1
    );
}

#[test]
fn simultaneous_loop_starts_use_document_order() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                event a 0.0 name="A"
            }
            loop second start=0.0 every=1.0 {
                event b 0.0 name="B"
            }
        }
    }
}
"###,
    )
    .unwrap();
    let input = recording(
        0,
        serde_json::json!([{ "at_ms":0, "type":"Start", "fields":{} }]),
    )
    .unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(!output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/first/a:")
        && r.at_ms == 0));
    assert!(output.iter().any(|r| r.action == "violation"
        && r.detail.contains("p/main/loop/second")
        && r.at_ms == 0));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/loop/second/b:"))
    );
}

#[test]
fn watching_and_exiting_a_future_loop_does_not_count_as_running() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop running start=0.0 every=2.0 {
                event pulse 0.0 name="Pulse"
            }
            loop waiting start=3.0 every=1.0 {
                until "Skip" at=1.0 before=0.0
                event never 0.0 name="Never"
            }
        }
    }
}
"###,
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
        !output
            .iter()
            .any(|r| r.action == "violation" || r.detail.starts_with("p/main/loop/waiting/never:"))
    );
}

#[test]
fn one_signal_matching_multiple_loop_exits_is_ambiguous() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop a start=5.0 every=1.0 {
                until "Skip" at=1.0 before=0.0
            }
            loop b start=6.0 every=1.0 {
                until "Skip" at=1.0 before=0.0
            }
        }
    }
}
"###,
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
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
                event a 0.0 name="A"
            }
            loop second start=2.0 every=2.0 {
                until "EndSecond" at=5.0 before=0.0
                event b 1.0 name="B" {
                    sync 1.0 "Sync" before=1.0 after=1.0
                }
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/second/b:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(b, [2500, 4500]);
    assert!(
        output
            .iter()
            .any(|r| r.action == "sync" && r.detail.starts_with("p/main/loop/second"))
    );
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.detail == "p/main/loop/second" && r.at_ms == 5000)
    );
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn phase_sync_skipping_multiple_loop_starts_blocks_the_later_loop() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 7.0 name="Anchor" {
                sync 7.0 "Sync" before=7.0 after=7.0
            }
            loop second start=3.0 every=2.0 {
                event b 0.0 name="B"
            }
            loop first start=2.0 every=2.0 {
                event a 0.0 name="A"
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "violation"
        && r.detail.contains("p/main/loop/second")
        && r.at_ms == 1000));
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "skipped" && r.detail == "p/main/loop/first/a")
            .count(),
        3
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/loop/second/b:"))
    );
    assert!(!output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/first/a:")
        && r.at_ms == 2000));
}

#[test]
fn second_loop_can_sync_at_the_same_timestamp_as_first_loop_exit() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=1.0 {
                until "EndFirst" at=2.0 before=0.0
            }
            loop second start=2.0 every=2.0 {
                event b 0.0 name="B" {
                    sync 0.0 "Sync" before=0.0 after=0.1
                }
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "sync"
        && r.detail.starts_with("p/main/loop/second")
        && r.at_ms == 2000));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/second/b:")
        && r.at_ms == 2000));
    assert!(!output.iter().any(|r| r.action == "violation"));
}

#[test]
fn blocked_loop_cannot_sync_at_its_start_boundary() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop first start=0.0 every=2.0 {
                event a 0.0 name="A"
            }
            loop second start=1.0 every=2.0 {
                event b 0.0 name="B" {
                    sync 0.0 "Sync" before=0.0 after=0.1
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "violation" && r.detail.contains("p/main/loop/second"))
    );
    assert!(!output.iter().any(|r| r.action == "sync"
        || (r.action == "event" && r.detail.starts_with("p/main/loop/second/b:"))));
}

#[test]
fn phase_and_loop_sync_matching_one_observation_are_ambiguous() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 3.0 name="Phase" {
                sync 1.0 "Sync" before=1.0 after=1.0
            }
            loop first start=0.0 every=2.0 {
                event a 0.0 name="A" {
                    sync 0.5 "Sync" before=1.0 after=1.0
                }
            }
            loop second start=10.0 every=2.0 {
                event b 0.0 name="B"
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 3000));
}

#[test]
fn loop_and_branch_can_run_in_the_same_phase() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch b open=0.0 close=3.0 {
                option a {
                    when "Choose"
                    event chosen 2.0 name="Chosen"
                }
                option b {
                    when "Other"
                }
            }
            loop l start=0.0 every=2.0 {
                until "Stop" at=3.0 before=0.0
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "option" && r.detail == "p/main/b/a")
    );
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/b/a/chosen:")
        && r.at_ms == 3000));
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
            .count(),
        2
    );
}

#[test]
fn simultaneous_branch_choice_and_loop_exit_are_ambiguous() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch b open=0.0 close=2.0 {
                option a {
                    when "X"
                }
                option b {
                    when "Other"
                }
            }
            loop l start=0.0 every=1.0 {
                until "X" at=1.0 before=1.0
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
            .count(),
        1
    );
}

#[test]
fn option_sync_moves_phase_and_exit_window_without_moving_active_loop() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 4.0 name="Phase"
            branch b open=0.0 close=2.0 {
                option a {
                    when "Choose"
                    event chosen 2.0 name="Chosen" {
                        sync 2.0 "Sync" before=2.0 after=2.0
                    }
                }
                option b {
                    when "Other"
                }
            }
            loop l start=0.0 every=2.0 {
                until "Stop" at=5.0 before=0.0 after=0.1
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [0, 2000]);
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/b/a/chosen:")
        && r.at_ms == 2000));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 3000));
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 4000)
    );
}

#[test]
fn loop_sync_moves_future_iterations_without_moving_phase() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 7.0 name="Phase"
            loop rotation start=2.0 every=4.0 {
                until "End" at=7.0 before=0.0 after=0.1
                event pulse 1.0 name="Pulse" {
                    sync 1.0 "Sync" before=1.0 after=1.0
                }
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/rotation/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [2500, 6500]);
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 7000));
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 7000)
    );
}

#[test]
fn loop_boundary_sync_precedes_zero_time_event() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=1.0 every=2.0 {
                event pulse 0.0 name="Pulse" {
                    sync 0.0 "Sync" before=0.0 after=0.1
                }
            }
        }
    }
}
"###,
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
            .filter(|r| r.action == "sync" && r.detail.starts_with("p/main/loop/rotation"))
            .count(),
        2
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/rotation/pulse:"))
            .count(),
        2
    );
}

#[test]
fn loop_exit_has_priority_over_iteration_sync() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            loop rotation start=1.0 every=2.0 {
                until "Stop" at=1.0 before=0.0
                event pulse 0.0 name="Pulse" {
                    sync 0.0 "Stop" before=0.0 after=0.1
                }
            }
        }
    }
}
"###,
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
fn phase_sync_does_not_move_an_active_iteration() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 6.0 name="Phase" {
                sync 5.0 "Sync" before=5.0 after=5.0
            }
            loop l start=0.0 every=2.0 {
                until "Stop" at=7.0 before=0.0 after=0.1
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [0, 2000]);
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 2000));
    assert!(
        output
            .iter()
            .any(|r| r.action == "until" && r.at_ms == 3000)
    );
}

#[test]
fn phase_sync_past_a_pending_loop_starts_the_current_iteration() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 5.0 name="Phase" {
                sync 5.0 "Sync" before=5.0 after=5.0
            }
            loop l start=3.0 every=2.0 {
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
            .filter(|r| r.action == "skipped" && r.detail == "p/main/loop/l/pulse")
            .count(),
        1
    );
    let pulses: Vec<_> = output
        .iter()
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [1000, 3000, 5000]);
}

#[test]
fn phase_sync_marks_skipped_iterations_and_partial_iteration_once() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 8.5 name="Anchor" {
                sync 8.5 "Forward" before=8.5 after=8.5
                sync 1.0 "Back" before=20.0 after=20.0
            }
            loop l start=3.0 every=2.0 {
                event zero 0.0 name="Zero"
                event middle 1.5 name="Middle"
            }
        }
    }
}
"###,
    )
    .unwrap();
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
        .filter(|r| {
            r.action == "skipped"
                && (r.detail == "p/main/loop/l/zero" || r.detail == "p/main/loop/l/middle")
        })
        .map(|r| r.detail.as_str())
        .collect();
    assert_eq!(
        skipped,
        [
            "p/main/loop/l/zero",
            "p/main/loop/l/middle",
            "p/main/loop/l/zero",
            "p/main/loop/l/middle",
            "p/main/loop/l/zero"
        ]
    );
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/l/middle:")
        && r.at_ms == 1000));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/l/zero:")
        && r.at_ms == 1500));
}

#[test]
fn phase_sync_rejects_an_excessive_number_of_skipped_loop_iterations() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 1000000.0 name="Anchor" {
                sync 1000000.0 "Forward" before=1000000.0 after=0.1
            }
            loop l start=1.0 every=0.1 {
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
    )
    .unwrap();
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
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 2.0 name="Anchor" {
                sync 2.0 "Sync" before=2.0 after=2.0
            }
            loop l start=5.0 every=2.0 {
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/loop/l/pulse:")
        && r.at_ms == 4000));
}

#[test]
fn phase_sync_at_loop_start_precedes_starting_the_loop() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 0.0 name="Anchor" {
                sync 0.0 "Sync" before=0.0 after=3.0
            }
            loop l start=2.0 every=2.0 {
                event pulse 0.0 name="Pulse"
            }
        }
    }
}
"###,
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
        .filter(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:"))
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(pulses, [4000]);
}

#[test]
fn phase_sync_reports_expired_exit_window_at_the_correction() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event anchor 5.0 name="Anchor" {
                sync 5.0 "Sync" before=5.0 after=5.0
            }
            loop l start=0.0 every=2.0 {
                until "Stop" at=2.0 before=0.0 after=0.1
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "unresolved" && r.detail == "p/main/loop/l" && r.at_ms == 1000)
    );
}

#[test]
fn phase_branch_preserves_conditions_and_schedules_for_replay() {
    let model = parse(
        r###"
timeline 2 "x" {
    phase p {
        step main {
            enter "Start"
            branch first open=0.0 close=5.0 {
                option left {
                    when "A" id="1"
                    event left-hit 1.0 name="Left"
                }
                option right {
                    when "B"
                }
                merge {
                    when "C" at=6.0 before=1.0 after=1.0
                    event shared 2.0 name="Shared"
                }
            }
            branch second open=7.0 close=9.0 {
                option one {
                    when "D"
                    requires "p/main/first" "left"
                }
                option two {
                    when "E"
                }
            }
        }
    }
}
"###,
    )
    .unwrap();
    let branches = &model.phases[0].branches;
    assert_eq!(branches.len(), 2);
    assert_eq!(
        branches[0].options[0].events[0].id,
        "p/main/first/left/left-hit"
    );
    assert!(branches[0].options[0].whens[0].matches("A", &[("id".into(), "1".into())].into(), 0));
    assert_eq!(
        branches[0].merge.as_ref().unwrap().events[0].id,
        "p/main/first/merge/shared"
    );
    assert_eq!(
        branches[1].options[0].requires,
        [("p/main/first".into(), "left".into())]
    );
}

#[test]
fn branch_select_sync_merge_and_shared_clock() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event phase-hit 5.0 name="Phase"
            branch b open=0.0 close=4.0 {
                option left {
                    when "Left"
                    event left-zero 0.0 name="Zero"
                    event left-hit 2.0 name="Left" {
                        sync 2.0 "Sync" before=3.0 after=3.0
                    }
                    event cancelled 5.0 name="Cancelled"
                }
                option right {
                    when "Right"
                }
                merge {
                    when "Merge"
                    event shared 1.0 name="Shared"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "option" && r.detail == "p/main/b/left")
    );
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/b/left/left-zero:")
        && r.at_ms == 1049));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/b/left/left-hit:")
        && r.at_ms == 2000));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/phase-hit:")
        && r.at_ms == 3951));
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("p/main/b/merge/shared:")
        && r.at_ms == 4000));
    assert!(
        !output
            .iter()
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/b/left/cancelled:"))
    );
}

#[test]
fn prior_choices_survive_phase_transition_but_not_wipe() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    wipe "Wipe"
    phase first {
        step main {
            enter "Start"
            branch first-choice open=0.0 close=2.0 {
                option a {
                    when "A"
                }
                option b {
                    when "B"
                }
            }
        }
    }
    phase second {
        step main {
            enter "Next"
            branch second-choice open=0.0 close=2.0 {
                option a {
                    when "A"
                    requires "first/main/first-choice" "a"
                }
                option b {
                    when "B"
                    requires "first/main/first-choice" "b"
                }
            }
        }
    }
}
"###,
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
        output.iter().any(|r| r.run == 1
            && r.action == "option"
            && r.detail == "second/main/second-choice/a")
    );
    assert!(output.iter().any(|r| r.run == 2
        && r.action == "unresolved"
        && r.detail.contains("missing prior choice")));
    assert!(
        !output.iter().any(|r| r.run == 2
            && r.action == "option"
            && r.detail == "second/main/second-choice/a")
    );
}

#[test]
fn closed_branch_does_not_reopen_after_backward_sync() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event sync 8.0 name="Sync" {
                sync 1.0 "Back" before=9.0 after=9.0
            }
            branch b open=0.0 close=2.0 {
                option a {
                    when "A"
                }
                option b {
                    when "B"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "unresolved" && r.detail == "p/main/b")
    );
    assert!(!output.iter().any(|r| r.action == "option"));
}

#[test]
fn competing_branch_actions_do_not_choose_or_merge() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch choice open=0.0 close=3.0 {
                option a {
                    when "X" {
                        regex "id" "A.*"
                    }
                }
                option b {
                    when "X" id="ABC"
                }
                merge {
                    when "X" id="ABC"
                }
            }
        }
    }
}
"###,
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
        !output
            .iter()
            .any(|r| r.action == "unresolved" && r.detail == "p/main/choice")
    );
    assert!(
        !output
            .iter()
            .any(|r| r.action == "option" || r.action == "merge")
    );
}

#[rstest]
#[case(1999, true)]
#[case(2000, false)]
fn selection_close_preserves_raw_phase_time(#[case] at_ms: i64, #[case] selected: bool) {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch b open=0.0 close=2.0 {
                option a {
                    when "A"
                }
                option b {
                    when "B"
                }
            }
        }
    }
}
"###,
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
            .any(|r| r.action == "unresolved" && r.detail == "p/main/b"),
        !selected
    );
}

#[test]
fn simultaneous_events_across_branch_and_phase_follow_document_order() {
    let model = parse(
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            branch b open=0.0 close=3.0 {
                option a {
                    when "A"
                    event branch-hit 1.0 name="Branch"
                }
                option b {
                    when "B"
                }
            }
            event phase-hit 2.0 name="Phase"
        }
    }
}
"###,
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
        matches!(events.as_slice(), [first, second] if first.starts_with("p/main/b/a/branch-hit:") && second.starts_with("p/main/phase-hit:"))
    );
}

#[test]
fn lifecycle_cancels_same_time_events_and_restarts_fresh() {
    let model = parse(include_str!("../../tests/fixtures/timeline/basic.kdl")).unwrap();
    let input: Recording =
        serde_json::from_str(include_str!("../../tests/fixtures/timeline/basic.json")).unwrap();
    let output = replay(&model, &input).unwrap();
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("opening/main/after-sync:")
        && r.at_ms == 6000));
    assert!(
        !output.iter().any(
            |r| r.action == "event" && r.detail.starts_with("opening/main/cancelled-by-phase:")
        )
    );
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("second/main/second-start:")
        && r.at_ms == 9000));
    assert!(output.iter().any(|r| r.action == "start" && r.run == 2));
    assert!(output.iter().any(|r| r.action == "unobserved"
        && r.detail == "second/main/beyond-recording"
        && r.run == 1
        && r.at_ms == 10500));
    assert!(
        output.iter().any(|r| r.action == "unobserved"
            && r.detail == "opening/main/opening-hit"
            && r.run == 2)
    );
}

#[test]
fn raw_order_rounding_ambiguity_and_forward_backward_sync() {
    let source = r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Start"
            event skipped 2.0 name="Skipped"
            event target 4.0 name="Target" {
                sync 4.0 "Forward" before=3.0 after=3.0
                sync 1.0 "Backward" before=5.0 after=5.0
            }
            event tail 5.0 name="Tail"
        }
    }
}
"###;
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
            .any(|r| r.action == "skipped" && r.detail == "p/main/skipped")
    );
    assert_eq!(
        output
            .iter()
            .filter(|r| r.action == "event" && r.detail.starts_with("p/main/target:"))
            .count(),
        1
    );
    assert_eq!(output.iter().filter(|r| r.action == "sync").count(), 2);
    assert!(!output.iter().any(|r| r.action == "ambiguous"));
    let ambiguous =
        parse(&source.replace("sync 1.0 \"Backward\"", "sync 1.0 \"Forward\"")).unwrap();
    assert!(
        !replay(&ambiguous, &input)
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
        r###"
timeline 2 "x" {
    start "Start"
    phase p {
        step main {
            enter "Enter" {
                field "id" "A" "B"
                regex "source" "Boss"
            }
            event e 0.0 name="Entry"
        }
    }
}
"###,
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
        r###"timeline 2 "Pattern" {{
        start "Start" {{ regex "source" #"{pattern}"#;	}}
        event "hit" 0.0 name="Hit"
}}"###
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
        r###"
timeline 2 "Valigarmanda post-add excerpt" {
    start "Ability" id="9565"
    phase after-adds {
        step main {
            enter "Ability" id="9565"
            event tulidisaster-first 16.0 name="Tulidisaster 1" {
                sync 3.9 "Ability" id="95B3" before=2.5 after=2.5
                sync 16.0 "Ability" id="8D67" before=2.5 after=2.5
            }
        }
    }
}
"###,
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
        && r.detail.starts_with("after-adds/main/tulidisaster-first:")
        && r.at_ms == 317000));
}

#[test]
fn raw_window_boundaries_and_input_shape_are_strict() {
    let model = parse(
        r###"
timeline 2 "Boundary" {
    start "Start"
    event "e" 2.0 name="Hit" {
        sync 1.0 "Signal" before=0.0 after=0.1
    }
}
"###,
    )
    .unwrap();
    for (at_ms, matches) in [(999, false), (1000, true), (1099, true), (1100, false)] {
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
        let source = format!(r###"timeline 2 "x" {{
            start "Start"
	phase p {{            step main {{
                enter "Start"
                event anchor {seconds} name="Anchor" {{ sync {seconds} "Sync" before={seconds} after=0.1;			}}
                loop l start=1.0 every=2.0 {{ event pulse 0.0 name="Pulse";			}}
		}}
	}}
}}"###);
        let model = parse(&source).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let input = recording(1000, serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Sync","fields":{}}
        ])).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let output = replay(&model, &input).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let offset = target - 10;
        let expected = offset / 20 + i64::from(offset % 20 > 0);
        let skipped = output.iter().filter(|r| r.action == "skipped" && r.detail == "p/main/loop/l/pulse").count();
        prop_assert_eq!(i64::try_from(skipped).unwrap_or(i64::MAX), expected);
        let at_target = output.iter().any(|r| r.action == "event" && r.detail.starts_with("p/main/loop/l/pulse:") && r.at_ms == 1000);
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
        let model = parse(r###"
timeline 2 "Corrections" {
    start "Start"
    event "early" 2.0 name="Early"
    event "target" 4.0 name="Target" {
        sync 4.0 "Forward" before=1000.0 after=1000.0
        sync 1.0 "Backward" before=1000.0 after=1000.0
    }
    event "tail" 5.0 name="Tail"
}
"###).map_err(|e| TestCaseError::fail(e.to_string()))?;
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
#[case::windows_touch_at_exclusive_boundary(
    "when \"Ability\" id=\"A\" at=2.0 before=1.0 after=1.0;",
    "when \"Ability\" id=\"A\" at=4.0 before=1.0 after=1.0;",
    false
)]
fn unconditional_literal_option_overlap(
    #[case] left: &str,
    #[case] right: &str,
    #[case] overlaps: bool,
) {
    let source = format!(
        r###"timeline 2 "Test" {{
	phase p {{
        step main {{
            enter "Start"
            branch choice open=0.0 close=10.0 {{
                option left {{ {left} 				}}
                option right {{ {right} 				}}
			}}
		}}
	}}
}}"###
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
    let source = r###"
timeline 2 "Choices" {
    phase p {
        step main {
            enter "Start"
            branch first open=0.0 close=5.0 {
                option left {
                    when "A"
                }
                option right {
                    when "B"
                }
            }
            branch second open=6.0 close=10.0 {
                option left {
                    requires "p/main/first" "left"
                    when "Ability" id="A"
                }
                option right {
                    requires "p/main/first" "right"
                    when "Ability" id="A"
                }
            }
        }
    }
}
"###;
    parse(source).unwrap();
}

#[test]
fn matching_signals_with_identical_prior_choices_are_rejected() {
    let source = r###"
timeline 2 "Choices" {
    phase p {
        step main {
            enter "Start"
            branch first open=0.0 close=5.0 {
                option left {
                    when "A"
                }
                option right {
                    when "B"
                }
            }
            branch second open=6.0 close=10.0 {
                option left {
                    requires "p/main/first" "left"
                    when "Ability" id="A"
                }
                option right {
                    when "Ability" id="A"
                    requires "p/main/first" "left"
                    requires "p/main/first" "left"
                }
            }
        }
    }
}
"###;
    let error = parse(source).unwrap_err();
    assert!(format!("{error:#}").contains("identical prerequisites"));
}

#[rstest]
#[case::identical_requirements("left", true)]
#[case::conflicting_requirements("right", false)]
fn option_cannot_require_two_choices_of_the_same_branch(#[case] second: &str, #[case] valid: bool) {
    let source = format!(
        r###"timeline 2 "x" {{
	phase p {{
        step main {{
            enter "Start"
            branch first open=0.0 close=1.0 {{
                option left {{ when "A";				}}
                option right {{ when "B";				}}
			}}
            branch next open=1.0 close=2.0 {{
                option one {{ when "C"; requires "p/main/first" "left"; requires "p/main/first" "{second}";				}}
                option two {{ when "D";				}}
			}}
		}}
	}}
}}"###
    );
    let result = parse(&source);
    assert_eq!(result.is_ok(), valid);
    if let Err(error) = result {
        assert!(format!("{error:#}").contains("conflicting choices for p/main/first"));
    }
}

#[rstest]
#[case::early(500)]
#[case::at_event(1000)]
fn wipe_records_censored_events_for_each_attempt(#[case] wipe_at: i64) {
    let model = parse(
        r###"
timeline 2 "Wipes" {
    start "Start"
    wipe "Wipe"
    event "fight-hit" 1.0 name="Fight hit"
    phase p {
        step main {
            enter "Start"
            event phase-hit 1.0 name="Phase hit"
        }
    }
}
"###,
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
        for id in ["fight-hit", "p/main/phase-hit"] {
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
        r###"
timeline 2 "Same timestamp" {
    start "Start"
    wipe "Wipe"
    phase p {
        step main {
            enter "Start"
            event hit 1.0 name="Hit"
        }
    }
}
"###,
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
            .any(|r| r.action == "event" && r.detail.starts_with("p/main/hit:"))
    );
    assert!(
        records
            .iter()
            .any(|r| r.action == "unobserved" && r.detail == "p/main/hit" && r.at_ms == 1000)
    );
}

#[rstest]
#[case(r#"on "StartsUsing" id="AD09"; event "hit" 3.0 name="Hit";"#, true)]
#[case(
    r#"on "StartsUsing" id="AD09" at=2.0; event "hit" 3.0 name="Hit";"#,
    false
)]
#[case(r#"event "hit" 3.0 name="Hit";"#, false)]
#[case(r#"on "StartsUsing" id="AD09";"#, false)]
#[case(
    r#"on "StartsUsing" id="AD09"; event "hit" 3.0 name="Hit"; event "hit" 4.0 name="Duplicate";"#,
    false
)]
fn validates_sequence_body_and_catalog_ownership(#[case] body: &str, #[case] valid: bool) {
    let source = format!(
        r###"timeline 2 "Example" {{ start "Ability" id="AD2B";	phase solo {{ step main {{
            enter "Ability" id="AD2B"
            reaction "mana" {{ {body} 			}}
		}} 	}}
}}"###
    );
    let result = parse(&source);
    assert_eq!(result.is_ok(), valid, "{result:?}");
    if let Ok(timeline) = result {
        let owner = &timeline.catalog.events.values().next().unwrap().owner;
        assert_eq!(owner.phase.as_deref(), Some("solo/main"));
        assert_eq!(owner.sequence.as_deref(), Some("solo/main/reaction/mana"));
    }
}

#[test]
fn sequence_ids_are_phase_local() {
    let phase = |id| {
        format!(
            r#"phase "{id}" {{
                step main {{
                enter "Ability" id="{id}"
                reaction "reaction" {{
                    on "StartsUsing" id="AD09"
                    event "{id}-hit" 3.0 name="Hit"
                }}
                }}
            }}"#
        )
    };
    let timeline = format!(
        r###"timeline 2 "Example" {{ start "Ability"; {}
{} }}"###,
        phase("one"),
        phase("two")
    );
    assert_eq!(parse(&timeline).unwrap().catalog.events.len(), 2);
    let duplicate = timeline.replace(
        r#"event "one-hit" 3.0 name="Hit"
                }"#,
        r#"event "one-hit" 3.0 name="Hit"
                }
                reaction "reaction" {
                    on "StartsUsing" id="AD08"
                    event "other-hit" 3.0 name="Hit"
                }"#,
    );
    let error = parse(&duplicate).expect_err("duplicate sequence in the same phase");
    assert!(format!("{error:#}").contains("Duplicate sequence ID one/main/reaction/reaction"));
}

const SEQUENCE_EXAMPLE: &str = r###"
timeline 2 "Independent sequences" {
    start "Start"
    wipe "Stop"
    phase solo {
        step main {
            enter "Start"
            event parent 5.0 name="Parent"
            reaction one {
                on "One"
                on "One" source="Boss"
                event first 1.0 name="First"
                event second 3.0 name="Second" {
                    sync 1.0 "Adjust" before=2.0 after=2.0
                    sync 1.0 "One" before=2.0 after=2.0
                }
            }
            reaction two {
                on "Two"
                event other 3.0 name="Other"
            }
        }
    }
    phase next {
        step main {
            enter "Change" from="solo/main" at=5.0 before=0.0 after=0.1
            reaction forbidden {
                on "Change"
                event must-not-start 0.0 name="Must not start"
            }
        }
    }
}
"###;

#[test]
fn sequence_restarts_keep_siblings_and_have_distinct_execution_identity() {
    let model = parse(SEQUENCE_EXAMPLE).unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":125,"type":"One","fields":{"source":"Boss"}},
            {"at_ms":500,"type":"Two","fields":{}},
            {"at_ms":1500,"type":"One","fields":{}}
        ]),
    )
    .unwrap();
    let before = super::forecast(&model, &input, 124, 5000, 10000).unwrap();
    assert_eq!(
        before
            .events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        ["solo/main/parent"]
    );
    let forecast = super::forecast(&model, &input, 1499, 4000, 10000).unwrap();
    assert!(
        forecast
            .events
            .iter()
            .any(|event| event.id == "solo/main/reaction/one/second" && event.at_ms == 3125)
    );
    let rows = replay(&model, &input).unwrap();
    assert!(!rows.iter().any(|row| row.action == "sync"));
    let sequence_starts = rows.iter().filter(|row| row.action == "sequence").count();
    assert_eq!(sequence_starts, 3);
    let events = rows
        .iter()
        .filter_map(|row| {
            let event = row.event.as_ref()?;
            Some((
                event.id.as_str(),
                row.at_ms,
                row.context.as_ref().unwrap().sequence_instance,
            ))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        [
            ("solo/main/reaction/one/first", 1125, Some(1)),
            ("solo/main/reaction/one/first", 2500, Some(3)),
            ("solo/main/reaction/two/other", 3500, Some(2)),
            ("solo/main/reaction/one/second", 4500, Some(3)),
            ("solo/main/parent", 5000, None),
        ]
    );
}

#[rstest]
#[case("Change", 5000, Some("next/main"))]
#[case("Stop", 4800, None)]
fn sequence_sync_is_local_and_parent_controls_cancel_pending_events(
    #[case] terminal: &str,
    #[case] at_ms: i64,
    #[case] final_phase: Option<&str>,
) {
    let model = parse(SEQUENCE_EXAMPLE).unwrap();
    let input = recording(
        6500,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"One","fields":{}},
            {"at_ms":1600,"type":"Two","fields":{}},
            {"at_ms":2700,"type":"Adjust","fields":{}},
            {"at_ms":4000,"type":"One","fields":{}},
            {"at_ms":at_ms,"type":terminal,"fields":{}}
        ]),
    )
    .unwrap();
    let forecast = super::forecast(&model, &input, 3000, 3000, 10000).unwrap();
    assert_eq!(
        forecast
            .events
            .iter()
            .map(|event| (event.id.as_str(), event.at_ms))
            .collect::<Vec<_>>(),
        [
            ("solo/main/reaction/two/other", 4600),
            ("solo/main/reaction/one/second", 4700),
            ("solo/main/parent", 5000)
        ]
    );
    let forecast = super::forecast(&model, &input, at_ms, 1000, 10000).unwrap();
    assert_eq!(forecast.phase.as_deref(), final_phase);
    assert!(forecast.events.is_empty());
    let rows = replay(&model, &input).unwrap();
    assert!(
        !rows
            .iter()
            .any(|row| row.at_ms >= at_ms && row.event.is_some())
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.action == "sequence" && row.at_ms == at_ms)
    );
}

#[test]
fn simultaneous_sequence_and_parent_sync_candidates_are_ambiguous() {
    let source = SEQUENCE_EXAMPLE.replace(
        r#"event parent 5.0 name="Parent""#,
        r#"event "parent" 5.0 name="Parent" {
            sync 2.0 "Adjust" before=2.0 after=2.0
        }"#,
    );
    let model = parse(&source).unwrap();
    let input = recording(
        6000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"One","fields":{}},
            {"at_ms":2700,"type":"Adjust","fields":{}}
        ]),
    )
    .unwrap();
    let forecast = super::forecast(&model, &input, 3000, 3000, 10000).unwrap();
    assert_eq!(forecast.diagnostics.len(), 1);
    assert_eq!(forecast.diagnostics[0].action, "ambiguous");
    assert_eq!(
        forecast
            .events
            .iter()
            .map(|event| (event.id.as_str(), event.at_ms))
            .collect::<Vec<_>>(),
        [
            ("solo/main/reaction/one/second", 4000),
            ("solo/main/parent", 5000)
        ]
    );
}

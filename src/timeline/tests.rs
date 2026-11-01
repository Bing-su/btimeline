use anyhow::Result;

use super::{Recording, parse, replay};

fn recording(end_ms: i64, observations: serde_json::Value) -> Result<Recording> {
    Ok(serde_json::from_value(
        serde_json::json!({"version":1,"end_ms":end_ms,"observations":observations}),
    )?)
}

#[test]
fn structure_rejects_lossy_times_unknown_nodes_fields_and_bad_references() {
    for source in [
        "timeline 2 \"x\" {}",
        "timeline 1 \"x\" { event \"e\" 0.01 name=\"x\"; }",
        "timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" visible=\"false\"; }",
        "timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" typo=#true; }",
        "timeline 1 \"x\" { event \"e\" 1.0 name=\"x\" name=\"y\"; }",
        "timeline 1 \"x\" { event \"e\" 1.0 name=\"x\"; event \"e\" 2.0 name=\"y\"; }",
        "timeline 1 \"x\" { phase \"p\" { enter \"X\" from=\"missing\"; }; }",
        "timeline 1 \"x\" { phase \"p\" { enter \"X\" at=1.0; }; }",
        "timeline 1 \"x\" { phase \"p\" { enter \"X\"; event \"e\" 1.0 name=\"x\" { sync 1.0 \"A\" before=1.0; }; }; }",
        "timeline 1 \"x\" { phase \"p\" { enter \"X\" id=\"A\" { field \"id\" \"B\"; }; }; }",
        "timeline 1 \"x\" { phase \"p\" { enter \"X\" { regex \"id\" \"[\"; }; }; }",
        "timeline 1 \"x\" { unknown; }",
    ] {
        assert!(parse(source).is_err(), "Accepted {source}");
    }
}

#[test]
fn branch_constraints_and_replay_capability_are_separate() -> Result<()> {
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
    let model = parse(body)?;
    assert_eq!(model.event_count, 3);
    let input = recording(0, serde_json::json!([]))?;
    assert!(replay(&model, &input).is_err());
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
    Ok(())
}

#[test]
fn lifecycle_cancels_same_time_events_and_restarts_fresh() -> Result<()> {
    let model = parse(include_str!("../../tests/fixtures/timeline/basic.kdl"))?;
    let input: Recording =
        serde_json::from_str(include_str!("../../tests/fixtures/timeline/basic.json"))?;
    let output = replay(&model, &input)?;
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
    assert!(
        output
            .iter()
            .any(|r| r.action == "unobserved" && r.detail == "opening-hit" && r.run == 2)
    );
    Ok(())
}

#[test]
fn raw_order_rounding_ambiguity_and_forward_backward_sync() -> Result<()> {
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
    let model = parse(source)?;
    let input = recording(
        7000,
        serde_json::json!([
            {"at_ms":0,"type":"Start","fields":{}},
            {"at_ms":1000,"type":"Forward","fields":{}},
            {"at_ms":1040,"type":"Backward","fields":{}}
        ]),
    )?;
    let output = replay(&model, &input)?;
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
    let ambiguous = parse(&source.replace("sync 1.0 \"Backward\"", "sync 1.0 \"Forward\""))?;
    assert!(
        replay(&ambiguous, &input)?
            .iter()
            .any(|r| r.action == "ambiguous")
    );
    let out_of_order = recording(
        7000,
        serde_json::json!([
            {"at_ms":100,"type":"Start","fields":{}},
            {"at_ms":99,"type":"Backward","fields":{}}
        ]),
    )?;
    assert!(replay(&model, &out_of_order).is_err());
    Ok(())
}

#[test]
fn idle_enter_is_ignored_and_regex_matches_whole_fields() -> Result<()> {
    let model = parse(
        r#"timeline 1 "x" {
        start "Start"
        phase "p" {
            enter "Enter" { field "id" "A" "B"; regex "source" "Boss"; }
            event "e" 0.0 name="Entry"
        }
    }"#,
    )?;
    let input = recording(
        5000,
        serde_json::json!([
            {"at_ms":0,"type":"Enter","fields":{"id":"A","source":"Boss"}},
            {"at_ms":1000,"type":"Start","fields":{}},
            {"at_ms":2000,"type":"Enter","fields":{"id":"A","source":"Other Boss"}},
            {"at_ms":3000,"type":"Enter","fields":{"id":"B","source":"Boss"}}
        ]),
    )?;
    let output = replay(&model, &input)?;
    let phase_times: Vec<_> = output
        .iter()
        .filter(|r| r.action == "phase")
        .map(|r| r.at_ms)
        .collect();
    assert_eq!(phase_times, [3000]);
    Ok(())
}

#[test]
fn ecmascript_backreferences_lookarounds_and_full_field_matching() -> Result<()> {
    use std::collections::BTreeMap;

    use regress::Regex;

    // Noncapturing wrapper preserves JS backreference numbering and forces alternative backtracking.
    for (pattern, accepted, rejected) in [
        (r"(Boss)\1", "BossBoss", "Boss"),
        (r"(?=Boss)Boss", "Boss", "Other Boss"),
        (r"a|ab", "ab", "abc"),
        (r"Boss", "Boss", "Boss\n"),
        (r"", "", "x"),
    ] {
        let signal = super::Signal {
            kind: "Ability".into(),
            fields: BTreeMap::from([(
                "source".into(),
                super::Field::Regex(vec![Regex::new(&format!("^(?:{pattern})(?![\\s\\S])"))?]),
            )]),
            at: None,
            before: 0,
            after: None,
            from: None,
        };
        assert!(signal.matches(
            "Ability",
            &BTreeMap::from([("source".into(), accepted.into())]),
            0
        ));
        assert!(!signal.matches(
            "Ability",
            &BTreeMap::from([("source".into(), rejected.into())]),
            0
        ));
    }
    let model = parse(
        r##"timeline 1 "Backreference" {
        start "Start" { regex "source" #"(Boss)\1"#; }
        event "hit" 0.0 name="Hit"
    }"##,
    )?;
    let input = recording(
        0,
        serde_json::json!([{ "at_ms":0, "type":"Start", "fields":{"source":"BossBoss"} }]),
    )?;
    assert!(replay(&model, &input)?.iter().any(|r| r.action == "event"));
    Ok(())
}

#[test]
fn source_based_post_adds_sync_moves_tulidisaster() -> Result<()> {
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
    )?;
    let input = recording(
        318000,
        serde_json::json!([
            {"at_ms":300000,"type":"Ability","fields":{"id":"9565"}},
            {"at_ms":304900,"type":"Ability","fields":{"id":"95B3"}}
        ]),
    )?;
    let output = replay(&model, &input)?;
    assert!(output.iter().any(|r| r.action == "event"
        && r.detail.starts_with("tulidisaster-first:")
        && r.at_ms == 317000));
    Ok(())
}

#[test]
fn quantized_window_boundaries_and_input_shape_are_strict() -> Result<()> {
    let model = parse(
        r#"timeline 1 "Boundary" {
        start "Start"
        event "e" 2.0 name="Hit" { sync 1.0 "Signal" before=0.0 after=0.0; }
    }"#,
    )?;
    for (at_ms, matches) in [(949, false), (950, true), (1049, true), (1050, false)] {
        let input = recording(
            3000,
            serde_json::json!([
                {"at_ms":0,"type":"Start","fields":{}},
                {"at_ms":at_ms,"type":"Signal","fields":{}}
            ]),
        )?;
        assert_eq!(
            replay(&model, &input)?.iter().any(|r| r.action == "sync"),
            matches
        );
    }
    for invalid in [
        serde_json::json!({"version":1,"end_ms":0,"observations":[],"unknown":true}),
        serde_json::json!({"version":1,"end_ms":0,"observations":[{"at_ms":0,"type":"Start","fields":{"id":1}}]}),
    ] {
        assert!(serde_json::from_value::<Recording>(invalid).is_err());
    }
    assert!(replay(&model, &recording(-1, serde_json::json!([]))?).is_err());
    Ok(())
}

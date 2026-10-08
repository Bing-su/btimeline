#![allow(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap to fail at the source"
)]

use std::collections::BTreeSet;
use std::fs;

use path_slash::PathBufExt as _;
use proptest::prelude::*;
use rstest::rstest;
use serde_json::{Value, json};

use super::*;
use crate::fflogs::model::{Ability, Event};
use crate::timeline::{ResetEvent, Timeline};

// Reuse collected-log fields for valid fixtures, e.g. omitted optional fields stay absent in JSON.
fn cast_event(timestamp: i64, source: i64, ability: i64, kind: &str) -> Event {
    Event {
        timestamp,
        kind: kind.into(),
        fight: Some(2),
        source_id: Some(source),
        target_id: None,
        source_instance: None,
        ability_game_id: Some(ability),
        melee: None,
        extra: Default::default(),
    }
}

fn multi_log(code: &str, rows: &[(i64, i64, i64, &str)], end: i64, kill: bool) -> Value {
    let mut data = sample();
    data["report"]["code"] = json!(code);
    data["collection"]["reportCode"] = json!(code);
    data["report"]["endTime"] = json!(1000 + end);
    data["report"]["fights"][0]["endTime"] = json!(1000 + end);
    data["report"]["fights"][0]["kill"] = json!(kill);
    data["collection"]["endTime"] = json!(1000 + end);
    data["collection"]["eventCount"] = json!(rows.len());
    data["report"]["masterData"]["abilities"] = serde_json::to_value(
        rows.iter()
            .map(|r| r.2)
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|&id| Ability {
                game_id: id,
                name: format!("Ability {id}"),
                kind: "1".into(),
                extra: Default::default(),
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    data["events"] = serde_json::to_value(
        rows.iter()
            .map(|&(at, actor, ability, kind)| cast_event(1000 + at, actor, ability, kind))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    data
}

// Three observed rounds make a real saving, e.g. A,B repeated becomes entry A plus B/continue A.
fn repeat_log(code: &str, width: usize, kill: bool) -> Value {
    let mut rows = vec![(1000, 10, 91000, "cast")];
    for round in 0..3 {
        rows.push((5000 + round * 10000, 10, 91001, "cast"));
        if width == 2 {
            rows.push((8000 + round * 10000, 11, 91002, "cast"));
        }
    }
    rows.extend([(38000, 10, 91003, "cast"), (42000, 10, 91004, "cast")]);
    let mut log = multi_log(code, &rows, 45000, kill);
    log["report"]["fights"][0]["encounterID"] = json!(99000 + width);
    log["report"]["fights"][0]["name"] = json!(format!("Unknown Repeat {width}"));
    log
}

#[rstest]
#[case::single_cast(1, GenerateMode::Raid)]
#[case::boss_and_helper(2, GenerateMode::Raid)]
#[case::alliance(2, GenerateMode::Alliance)]
#[case::dungeon(2, GenerateMode::Dungeon)]
fn p8_conditional_repeat_preserves_rounds_exit_and_holdout(
    #[case] width: usize,
    #[case] mode: GenerateMode,
) {
    let clear = repeat_log("clear", width, true);
    let wipe = repeat_log("wipe", width, false);
    let mut truncated = repeat_log("early-wipe", width, false);
    let count = if width == 2 { 4 } else { 3 };
    truncated["events"].as_array_mut().unwrap().truncate(count);
    truncated["collection"]["eventCount"] = json!(count);
    truncated["report"]["endTime"] = json!(18000);
    truncated["report"]["fights"][0]["endTime"] = json!(18000);
    truncated["collection"]["endTime"] = json!(18000);
    with_logs(&[clear.clone(), wipe, truncated], |dir| {
        let yaml = dir.join("repeat.yaml");
        generate(dir, &yaml, mode).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap()).unwrap();
        assert_eq!(report["repeats"]["accepted"], true, "{}", report["repeats"]);
        assert_eq!(report["validation"]["replay"], true);
        assert_eq!(report["validation"]["runtime"], false);
        assert_eq!(report["slots"].as_array().unwrap().len(), width + 4);
        assert_eq!(
            report["repeats"]["candidates"][0]["period"]["medianMs"],
            10000.0
        );
        assert_eq!(
            report["repeats"]["candidates"][0]["exitOffset"]["medianMs"],
            13000.0
        );
        for check in report["repeats"]["checks"].as_array().unwrap() {
            assert_eq!(check["passed"], true);
            if check["file"].as_str().unwrap().ends_with("2.json") {
                assert!(check["summary"]["censored"].as_u64().unwrap() > 0);
            } else {
                assert_eq!(check["summary"]["matches"], width * 3 + 3);
                assert_eq!(check["jumps"].as_array().unwrap().len(), 4);
            }
        }
        let repeat_yaml = fs::read_to_string(&yaml).unwrap();
        let timeline: Timeline = serde_saphyr::from_str(&repeat_yaml).unwrap();
        assert_eq!(
            timeline.reset_on.contains(&ResetEvent::AreaClear),
            mode != GenerateMode::Raid
        );
        assert_eq!(
            timeline.reset_on.contains(&ResetEvent::Wipe),
            mode != GenerateMode::Dungeon
        );
        let (same_yaml, same_report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            mode,
            30.0,
        )
        .unwrap();
        assert_eq!(same_yaml, repeat_yaml);
        assert_eq!(same_report, report);
        let holdout = dir.join("holdout.json");
        fs::write(
            &holdout,
            serde_json::to_vec(&repeat_log("holdout", width, true)).unwrap(),
        )
        .unwrap();
        let output = dir.join("holdout.replay.json");
        replay::replay_file(&yaml, &holdout, &output, None).unwrap();
        let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(result["pulls"][0]["unrepresentedEventIndices"], json!([]));
        assert_eq!(
            result["pulls"][0]["replay"]["summary"]["matches"],
            width * 3 + 3
        );
        // A clear during the second round truncates future repeats and exit rather than inventing them.
        let mut early = repeat_log("early-holdout", width, true);
        early["events"].as_array_mut().unwrap().truncate(count);
        early["collection"]["eventCount"] = json!(count);
        early["report"]["endTime"] = json!(18000);
        early["report"]["fights"][0]["endTime"] = json!(18000);
        early["collection"]["endTime"] = json!(18000);
        fs::write(&holdout, serde_json::to_vec(&early).unwrap()).unwrap();
        replay::replay_file(&yaml, &holdout, dir.join("early.replay.json"), None).unwrap();
        // Replaying a changed helper instance must fail even when IDs and timing remain identical.
        let mut changed = repeat_log("changed", width, true);
        changed["events"][2]["sourceInstance"] = json!(99);
        fs::write(&holdout, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(
            replay::replay_file(&yaml, &holdout, dir.join("changed.replay.json"), None).is_err()
        );
    });
}

#[test]
fn p8_exit_before_continuation_keeps_sorted_slot_evidence() {
    let mut clear = repeat_log("clear", 2, true);
    let mut wipe = repeat_log("wipe", 2, false);
    for log in [&mut clear, &mut wipe] {
        log["events"][7]["timestamp"] = json!(34000);
    }
    with_logs(&[clear, wipe], |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["repeats"]["accepted"], true, "{}", report["repeats"]);
        let events = draft_events(&yaml);
        let exit = events
            .iter()
            .position(|event| event["jump"]["to"] == "repeat-exit-0")
            .unwrap();
        let continuation = events
            .iter()
            .rposition(|event| event["jump"]["to"] == "repeat-0")
            .unwrap();
        assert!(exit < continuation);
        assert_eq!(report["slots"][exit]["abilityIds"], json!([91003]));
        assert_eq!(report["slots"][continuation]["abilityIds"], json!([91001]));
        for check in report["repeats"]["checks"].as_array().unwrap() {
            assert_eq!(check["passed"], true);
        }
    });
}

// Preserve finite failures across exit jumps, e.g. one observed round cannot stand in for three.
#[rstest]
#[case(1)]
#[case(2)]
fn p8_early_exit_cannot_hide_missing_rounds(#[case] rounds: usize) {
    let mut clear = repeat_log("clear", 1, true);
    let mut wipe = repeat_log("wipe", 1, false);
    for log in [&mut clear, &mut wipe] {
        log["events"][4]["timestamp"] = json!(34000);
        log["events"][5]["timestamp"] = json!(38000);
    }
    with_logs(&[clear.clone(), wipe], |dir| {
        let yaml = dir.join("repeat.yaml");
        generate(dir, &yaml, GenerateMode::Raid).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap()).unwrap();
        assert_eq!(report["repeats"]["accepted"], true);
        let mut holdout = clear;
        holdout["report"]["code"] = json!("holdout");
        holdout["collection"]["reportCode"] = json!("holdout");
        holdout["events"]
            .as_array_mut()
            .unwrap()
            .drain(1 + rounds..4);
        let exit = 1 + rounds;
        let shift = (3 - rounds) * 10000;
        holdout["events"][exit]["timestamp"] = json!(34000 - shift);
        holdout["events"][exit + 1]["timestamp"] = json!(38000 - shift);
        holdout["collection"]["eventCount"] = json!(holdout["events"].as_array().unwrap().len());
        let input = dir.join("holdout.json");
        fs::write(&input, serde_json::to_vec(&holdout).unwrap()).unwrap();
        let output = dir.join("holdout.replay.json");
        assert!(replay::replay_file(&yaml, &input, &output, None).is_err());
        let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert!(
            result["pulls"][0]["replay"]["summary"]["missing"]
                .as_u64()
                .unwrap()
                > 0
        );
    });
}

// Respect source order at equal timestamps, e.g. an update after A cannot change A's context.
#[rstest]
#[case::after_cast(false)]
#[case::before_cast(true)]
fn p8_holdout_targetability_uses_state_before_cast(#[case] update_before_cast: bool) {
    let mut clear = repeat_log("clear", 2, true);
    let mut wipe = repeat_log("wipe", 2, false);
    for log in [&mut clear, &mut wipe] {
        log["events"].as_array_mut().unwrap().push(json!({"timestamp":1000,"fight":2,"type":"targetabilityupdate","sourceID":10,"targetID":10,"targetable":1}));
        log["collection"]["eventCount"] = json!(10);
    }
    with_logs(&[clear.clone(), wipe], |dir| {
        let yaml = dir.join("repeat.yaml");
        generate(dir, &yaml, GenerateMode::Raid).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap()).unwrap();
        assert_eq!(report["repeats"]["accepted"], true);
        let mut holdout = clear;
        holdout["report"]["code"] = json!("holdout");
        holdout["collection"]["reportCode"] = json!("holdout");
        holdout["events"].as_array_mut().unwrap().extend([
            json!({"timestamp":15999,"fight":2,"type":"targetabilityupdate","sourceID":10,"targetID":10,"targetable":0}),
            json!({"timestamp":16000,"fight":2,"type":"targetabilityupdate","sourceID":10,"targetID":10,"targetable":1}),
        ]);
        if update_before_cast {
            let events = holdout["events"].as_array_mut().unwrap();
            let update = events.pop().unwrap();
            events.insert(3, update);
        }
        holdout["collection"]["eventCount"] = json!(12);
        let input = dir.join("holdout.json");
        fs::write(&input, serde_json::to_vec(&holdout).unwrap()).unwrap();
        let output = dir.join("holdout.replay.json");
        assert_eq!(
            replay::replay_file(&yaml, &input, &output, None).is_ok(),
            update_before_cast
        );
        assert!(output.exists());
    });
}

// Inspect the final body too, e.g. a new helper start between its A and B invalidates compression.
#[rstest]
#[case(0, 8000)]
#[case(1, 8000)]
#[case(2, 8000)]
#[case(2, 10000)]
fn p8_each_round_rejects_additional_helper_start(#[case] round: i64, #[case] at: i64) {
    let clear = repeat_log("clear", 2, true);
    let mut wipe = repeat_log("wipe", 2, false);
    wipe["events"].as_array_mut().unwrap().push(json!({"timestamp":at + round * 10000,"fight":2,"type":"begincast","sourceID":11,"abilityGameID":91004}));
    wipe["collection"]["eventCount"] = json!(10);
    with_logs(&[clear, wipe], |dir| {
        let (_, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["repeats"]["accepted"], false);
        assert!(
            report["repeats"]["candidates"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("order")
        );
    });
}

// Keep the exit's own start distinct, e.g. E begins after the final B and then completes normally.
#[test]
fn p8_final_round_allows_paired_exit_start() {
    let mut clear = repeat_log("clear", 2, true);
    let mut wipe = repeat_log("wipe", 2, false);
    for log in [&mut clear, &mut wipe] {
        log["events"].as_array_mut().unwrap().push(json!({"timestamp":36000,"fight":2,"type":"begincast","sourceID":10,"abilityGameID":91003}));
        log["collection"]["eventCount"] = json!(10);
    }
    with_logs(&[clear, wipe], |dir| {
        let (_, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["repeats"]["accepted"], true, "{}", report["repeats"]);
    });
}

#[test]
fn p8_disabled_suffix_diagnostic_uses_compressed_slot() {
    let mut clear = repeat_log("clear", 2, true);
    let mut wipe = repeat_log("wipe", 2, false);
    for log in [&mut clear, &mut wipe] {
        log["events"].as_array_mut().unwrap().push(json!({"timestamp":42900,"fight":2,"type":"cast","sourceID":10,"abilityGameID":91004,"melee":true}));
        log["collection"]["eventCount"] = json!(10);
    }
    with_logs(&[clear, wipe], |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["repeats"]["accepted"], true, "{}", report["repeats"]);
        let events = draft_events(&yaml);
        let suffix = events.len() - 1;
        assert_eq!(report["syncConflicts"][0]["slot"], suffix);
        assert!(
            events.last().unwrap()["note"]
                .as_str()
                .unwrap()
                .contains(&format!("see slot {suffix}"))
        );
        assert_eq!(events.last().unwrap()["sync"]["enabled"], false);
    });
}

#[rstest]
#[case::no_exit("exit")]
#[case::only_clears("clears")]
#[case::changed_instance("instance")]
#[case::changed_targetability("targetability")]
#[case::raw_competitor("competitor")]
fn p8_rejected_repeat_keeps_finite_rows_and_reports_reason(#[case] scenario: &str) {
    let mut a = repeat_log("a", 2, true);
    let mut b = repeat_log("b", 2, false);
    match scenario {
        "exit" => {
            for log in [&mut a, &mut b] {
                log["events"].as_array_mut().unwrap().truncate(7);
                log["collection"]["eventCount"] = json!(7);
            }
        }
        "clears" => b["report"]["fights"][0]["kill"] = true.into(),
        "instance" => b["events"][4]["sourceInstance"] = json!(2),
        "targetability" => {
            b["events"].as_array_mut().unwrap().push(json!({"timestamp":20000,"fight":2,"type":"targetabilityupdate","sourceID":10,"targetID":10,"targetable":0}));
            b["collection"]["eventCount"] = json!(10);
        }
        "competitor" => {
            // An excluded melee cast still competes with the active repeat selector in raw replay.
            b["events"].as_array_mut().unwrap().push(json!({"timestamp":15900,"fight":2,"type":"cast","sourceID":10,"abilityGameID":91001,"melee":true}));
            b["collection"]["eventCount"] = json!(10);
        }
        _ => assert!(scenario.is_empty(), "unknown repeat scenario"),
    }
    with_logs(&[a, b], |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(
            report["repeats"]["accepted"], false,
            "{}",
            report["repeats"]
        );
        assert!(
            !report["repeats"]["candidates"][0]["reason"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(
            draft_events(&yaml)
                .iter()
                .all(|entry| entry.get("jump").is_none())
        );
    });
}

#[rstest]
#[case::first_path(90002, 90005)]
#[case::second_path(90003, 90006)]
fn p7_branch_holdout_selects_one_path_merges_and_projects_only_its_future(
    #[case] selector: i64,
    #[case] successor: i64,
) {
    let a = [
        (1000, 10, 90001, "cast"),
        (5000, 11, 90002, "cast"),
        (6000, 11, 90007, "cast"),
        (7000, 11, 90005, "cast"),
        (9000, 10, 90004, "cast"),
        (11000, 10, 90008, "cast"),
    ];
    let b = [
        (2000, 10, 90001, "cast"),
        (12000, 11, 90003, "cast"),
        (13000, 11, 90007, "cast"),
        (14000, 11, 90006, "cast"),
        (16000, 10, 90004, "cast"),
        (18000, 10, 90008, "cast"),
    ];
    with_logs(
        &[
            multi_log("a", &a, 20000, true),
            multi_log("b", &b, 20000, true),
        ],
        |dir| {
            let yaml = dir.join("branch.yaml");
            generate(dir, &yaml, GenerateMode::Raid).unwrap();
            let report: Value =
                serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["extensions"]["accepted"], true);
            assert_eq!(report["extensions"]["forcejumpGenerated"], false);
            // Keep public evidence keys and omission semantics, e.g. common rows have no path marker.
            assert!(report["slots"][0].get("path").is_none());
            assert!(report["slots"][0].get("selector").is_none());
            assert!(report["blocks"][0].get("label").is_none());
            assert!(report["extensions"].get("reason").is_none());
            for check in report["extensions"]["checks"].as_array().unwrap() {
                assert_eq!(check["passed"], true);
                assert_eq!(check["jumps"].as_array().unwrap().len(), 2);
                assert_eq!(check["previews"].as_array().unwrap().len(), 4);
            }
            let holdout = multi_log(
                "holdout",
                &[
                    (1500, 10, 90001, "cast"),
                    (9000, 11, selector, "cast"),
                    (10000, 11, 90007, "cast"),
                    (11000, 11, successor, "cast"),
                    (13000, 10, 90004, "cast"),
                    (15000, 10, 90008, "cast"),
                ],
                16000,
                true,
            );
            // Independent evidence must not learn from the holdout, e.g. its timing falls between train paths.
            let input = dir.join("holdout.json");
            fs::write(&input, serde_json::to_vec(&holdout).unwrap()).unwrap();
            let output = dir.join("branch.replay.json");
            replay::replay_file(&yaml, &input, &output, None).unwrap();
            let replay: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            let run = &replay["pulls"][0]["replay"];
            assert_eq!(run["summary"]["matches"], 6);
            assert_eq!(run["summary"]["wrongMatches"], 0);
            assert_eq!(run["summary"]["ambiguousMatches"], 0);
            assert_eq!(run["jumps"].as_array().unwrap().len(), 2);
            let timeline: Value =
                serde_saphyr::from_str(&fs::read_to_string(&yaml).unwrap()).unwrap();
            let previews = run["previews"].as_array().unwrap();
            let future = &previews
                .iter()
                .find(|p| p["moment"] == "afterJump")
                .unwrap()["entryIndices"];
            let names: Vec<_> = future
                .as_array()
                .unwrap()
                .iter()
                .map(|i| {
                    timeline["entries"][i.as_u64().unwrap() as usize]["name"]
                        .as_str()
                        .unwrap()
                })
                .collect();
            assert!(names.contains(&format!("Ability {successor}").as_str()));
            assert!(!names.contains(
                &format!("Ability {}", if successor == 90005 { 90006 } else { 90005 }).as_str()
            ));
            assert_eq!(replay["validation"]["runtime"], false);
            // A new selector and a new successor must fail with zero accidental path activation.
            let unknown = multi_log(
                "unknown",
                &[
                    (1500, 10, 90001, "cast"),
                    (9000, 11, 90009, "cast"),
                    (11000, 11, 90010, "cast"),
                    (13000, 10, 90004, "cast"),
                    (15000, 10, 90008, "cast"),
                ],
                16000,
                true,
            );
            fs::write(&input, serde_json::to_vec(&unknown).unwrap()).unwrap();
            let output = dir.join("unknown.replay.json");
            assert!(replay::replay_file(&yaml, &input, &output, None).is_err());
            let unknown: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            assert_eq!(unknown["status"], "draft");
            assert_eq!(unknown["pulls"][0]["replay"]["summary"]["wrongMatches"], 0);
            assert_eq!(unknown["pulls"][0]["replay"]["jumps"], json!([]));
            // A familiar selector with an unknown dependent successor cannot pass either.
            let changed = multi_log(
                "changed",
                &[
                    (1500, 10, 90001, "cast"),
                    (9000, 11, selector, "cast"),
                    (10000, 11, 90007, "cast"),
                    (11000, 11, 90010, "cast"),
                    (13000, 10, 90004, "cast"),
                    (15000, 10, 90008, "cast"),
                ],
                16000,
                true,
            );
            fs::write(&input, serde_json::to_vec(&changed).unwrap()).unwrap();
            assert!(
                replay::replay_file(&yaml, &input, dir.join("changed.replay.json"), None).is_err()
            );
        },
    );
}

#[rstest]
#[case::wipe_before_choice(1500)]
#[case::wipe_inside_path(2500)]
fn p7_branch_wipe_does_not_create_an_empty_path(#[case] end: i64) {
    let a = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90002, "cast"),
        (3000, 11, 90005, "cast"),
        (4000, 10, 90004, "cast"),
    ];
    let b = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90003, "cast"),
        (3000, 11, 90006, "cast"),
        (4000, 10, 90004, "cast"),
    ];
    let wipe: Vec<_> = a.iter().filter(|r| r.0 < end).copied().collect();
    with_logs(
        &[
            multi_log("a", &a, 5000, true),
            multi_log("b", &b, 5000, true),
            multi_log("wipe", &wipe, end, false),
        ],
        |dir| {
            let yaml = dir.join("draft.yaml");
            generate(dir, &yaml, GenerateMode::Raid).unwrap();
            let report: Value =
                serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["extensions"]["accepted"], true);
            assert_eq!(
                report["extensions"]["branches"][0]["paths"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            replay::replay_file(&yaml, dir, dir.join("wipe.replay.json"), None).unwrap();
        },
    );
}

#[rstest]
#[case::raid(GenerateMode::Raid)]
#[case::alliance(GenerateMode::Alliance)]
#[case::dungeon(GenerateMode::Dungeon)]
fn p7_phase_window_uses_corrected_clock_and_rejects_out_of_sample_arrival(
    #[case] mode: GenerateMode,
) {
    let a = [
        (1000, 10, 90001, "cast"),
        (5000, 10, 90002, "cast"),
        (6000, 10, 90003, "cast"),
    ];
    let b = [
        (2000, 10, 90001, "cast"),
        (16000, 10, 90002, "cast"),
        (17000, 10, 90003, "cast"),
    ];
    with_logs(
        &[
            multi_log("a", &a, 20000, true),
            multi_log("b", &b, 20000, true),
        ],
        |dir| {
            let yaml = dir.join("phase.yaml");
            generate(dir, &yaml, mode).unwrap();
            let timeline: Timeline =
                serde_saphyr::from_str(&fs::read_to_string(&yaml).unwrap()).unwrap();
            assert_eq!(
                timeline.reset_on.contains(&ResetEvent::AreaClear),
                mode != GenerateMode::Raid
            );
            assert_eq!(
                timeline.reset_on.contains(&ResetEvent::Wipe),
                mode != GenerateMode::Dungeon
            );
            let entries = draft_events(&fs::read_to_string(&yaml).unwrap());
            assert_eq!(entries[1]["at"], 10.5);
            assert_eq!(entries[1]["sync"]["window"], json!([5.0, 5.1]));
            assert_eq!(entries[1]["jump"]["when"], "sync");
            let report: Value =
                serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["slots"][1]["clockTime"]["minMs"], 5500);
            assert_eq!(report["slots"][1]["clockTime"]["maxMs"], 15500);
            assert_eq!(report["extensions"]["phases"].as_array().unwrap().len(), 1);
            for (name, arrival, passed) in [
                ("early", 5500, true),
                ("late", 15500, true),
                ("outside", 15601, false),
            ] {
                let holdout = multi_log(
                    name,
                    &[
                        (1500, 10, 90001, "cast"),
                        (arrival, 10, 90002, "cast"),
                        (arrival + 1000, 10, 90003, "cast"),
                    ],
                    20000,
                    true,
                );
                let input = dir.join(format!("{name}.json"));
                fs::write(&input, serde_json::to_vec(&holdout).unwrap()).unwrap();
                assert_eq!(
                    replay::replay_file(
                        &yaml,
                        &input,
                        dir.join(format!("{name}.replay.json")),
                        None
                    )
                    .is_ok(),
                    passed
                );
            }
        },
    );
}

#[test]
fn p7_raw_discriminator_collision_refuses_the_extension() {
    let a = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90002, "cast"),
        (3000, 11, 90005, "cast"),
        (4000, 10, 90004, "cast"),
    ];
    let b = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90003, "cast"),
        (3000, 11, 90006, "cast"),
        (4000, 10, 90004, "cast"),
    ];
    let mut collision = multi_log("a", &a, 5000, true);
    // A friendly raw event with the same visible name is outside normalized alignment but can select X.
    let mut actor = collision["report"]["masterData"]["actors"][1].clone();
    actor["id"] = json!(12);
    collision["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(actor);
    collision["events"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(cast_event(3000, 12, 90003, "cast")).unwrap());
    collision["collection"]["eventCount"] = json!(5);
    with_logs(&[collision, multi_log("b", &b, 5000, true)], |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["extensions"]["accepted"], false);
        // Rejected candidates expose diagnostics only, e.g. no accepted branch or phase metadata.
        assert!(report["extensions"]["reason"].is_string());
        assert!(report["extensions"].get("branches").is_none());
        assert!(report["extensions"].get("phases").is_none());
        assert!(draft_events(&yaml).iter().all(|e| e["jump"].is_null()));
        assert!(
            report["extensions"]["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["summary"]["ambiguousMatches"].as_u64().unwrap() > 0)
        );
    });
}

#[test]
fn p7_shared_prefix_and_configured_lookahead_preserve_deterministic_blocks() {
    let a = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90007, "cast"),
        (3000, 11, 90002, "cast"),
        (4000, 11, 90005, "cast"),
        (5000, 10, 90004, "cast"),
    ];
    let b = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90007, "cast"),
        (3000, 11, 90003, "cast"),
        (4000, 11, 90006, "cast"),
        (5000, 10, 90004, "cast"),
    ];
    with_logs(
        &[
            multi_log("a", &a, 6051, true),
            multi_log("b", &b, 6051, true),
        ],
        |dir| {
            let group = input::select_group(dir, None, None, None).unwrap();
            let (yaml, report) = multi::build(group, GenerateMode::Raid, 60.0).unwrap();
            assert_eq!(report["extensions"]["accepted"], true);
            assert_eq!(report["extensions"]["lookaheadMs"], 60000);
            assert_eq!(
                draft_events(&yaml)
                    .iter()
                    .filter(|e| e["name"] == "Ability 90007")
                    .count(),
                1
            );
            let mut reversed = input::select_group(dir, None, None, None).unwrap();
            reversed.pulls.reverse();
            let (other_yaml, other_report) =
                multi::build(reversed, GenerateMode::Raid, 60.0).unwrap();
            assert_eq!(yaml, other_yaml);
            assert_eq!(report, other_report);
            let markdown = report::render(&report).unwrap();
            assert!(markdown.contains("분기·페이즈 확장"));
            assert!(markdown.contains("60000"));
            assert!(markdown.contains("실제 runtime 표시 | 미실행"));
            for invalid in [f64::NAN, -1.0, 3601.0] {
                assert!(
                    draft::generate_selected(
                        dir,
                        dir.join("bad.yaml"),
                        GenerateMode::Raid,
                        None,
                        None,
                        None,
                        invalid
                    )
                    .is_err()
                );
                assert!(!dir.join("bad.yaml").exists());
            }
        },
    );
}

#[test]
fn p7_safe_common_phase_survives_a_rejected_branch() {
    let a = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90002, "cast"),
        (3000, 11, 90005, "cast"),
        (4000, 10, 90004, "cast"),
        (5000, 10, 90008, "cast"),
        (6000, 10, 90009, "cast"),
    ];
    let b = [
        (1000, 10, 90001, "cast"),
        (2000, 11, 90003, "cast"),
        (3000, 11, 90006, "cast"),
        (4000, 10, 90004, "cast"),
        (15000, 10, 90008, "cast"),
        (16000, 10, 90009, "cast"),
    ];
    let mut collision = multi_log("a", &a, 20000, true);
    let mut actor = collision["report"]["masterData"]["actors"][1].clone();
    actor["id"] = json!(12);
    collision["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(actor);
    collision["events"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(cast_event(3000, 12, 90003, "cast")).unwrap());
    collision["collection"]["eventCount"] = json!(7);
    with_logs(&[collision, multi_log("b", &b, 20000, true)], |dir| {
        let yaml = dir.join("phase.yaml");
        generate(dir, &yaml, GenerateMode::Raid).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(yaml.with_extension("report.json")).unwrap()).unwrap();
        assert_eq!(report["extensions"]["accepted"], true);
        assert_eq!(report["extensions"]["branches"], json!([]));
        assert_eq!(report["extensions"]["phases"].as_array().unwrap().len(), 1);
        assert_eq!(
            report["extensions"]["rejectedBranchCandidate"]["accepted"],
            false
        );
        assert_eq!(draft_events(&fs::read_to_string(&yaml).unwrap()).len(), 4);
        replay::replay_file(&yaml, dir, dir.join("phase.replay.json"), None).unwrap();
    });
}

#[rstest]
#[case(false)]
#[case(true)]
fn replay_generated_unknown_encounter_and_holdout_preserves_termination(#[case] kill: bool) {
    let training = multi_log(
        "train",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90001, "cast"),
            (9000, 10, 90002, "cast"),
        ],
        10000,
        true,
    );
    let holdout = multi_log(
        "holdout",
        &[
            (1100, 10, 90001, "cast"),
            (5100, 10, 90001, "cast"),
            (5500, 10, 90002, "begincast"),
        ],
        6000,
        kill,
    );
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, GenerateMode::Raid).unwrap();
        let before = fs::read(&yaml).unwrap();
        let output = dir.join("replay.json");
        replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(report["status"], "internally_validated");
        assert_eq!(
            report["pulls"][0]["termination"],
            if kill { "kill" } else { "wipe" }
        );
        assert_eq!(report["pulls"][0]["replay"]["summary"]["matches"], 2);
        assert_eq!(report["pulls"][0]["replay"]["summary"]["censored"], 1);
        assert_eq!(report["pulls"][0]["unfinishedStarts"], json!([2]));
        assert_eq!(report["validation"]["cactbotParser"], false);
        assert_eq!(fs::read(&yaml).unwrap(), before);
        assert!(replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).is_err());
    });
}

#[test]
fn replay_retains_failure_report_and_rejects_changed_evidence() {
    let training = multi_log(
        "train",
        &[(1000, 10, 90001, "cast"), (5000, 10, 90002, "cast")],
        10000,
        true,
    );
    let holdout = multi_log(
        "holdout",
        &[(1000, 10, 90001, "cast"), (9000, 10, 90002, "cast")],
        10000,
        true,
    );
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, GenerateMode::Raid).unwrap();
        let output = dir.join("failure.json");
        assert!(replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).is_err());
        let result: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(result["validation"]["replayExecuted"], true);
        assert_eq!(result["validation"]["replay"], false);
        assert_eq!(result["pulls"][0]["replay"]["summary"]["windowMisses"], 1);
        assert!(output.with_extension("md").exists());
        let source = dir.join("fight_0.json");
        let mut changed: Value = serde_json::from_slice(&fs::read(&source).unwrap()).unwrap();
        changed["report"]["revision"] = json!(2);
        fs::write(&source, serde_json::to_vec(&changed).unwrap()).unwrap();
        let rejected = dir.join("rejected.json");
        assert!(replay::replay_file(&yaml, dir.join("fight_1.json"), &rejected, None).is_err());
        assert!(!rejected.exists());
    });
}

#[rstest]
#[case(false, 500)]
#[case(true, 250)]
fn replay_observed_successor_keeps_interior_missing_until_jump_skips_it(
    #[case] kill: bool,
    #[case] end: i64,
) {
    let training = multi_log(
        "train",
        &[
            (100, 10, 90001, "cast"),
            (300, 11, 90002, "cast"),
            (400, 10, 90003, "cast"),
        ],
        500,
        true,
    );
    let holdout = multi_log(
        "holdout",
        &[(100, 10, 90001, "cast"), (200, 10, 90003, "cast")],
        end,
        kill,
    );
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, GenerateMode::Raid).unwrap();
        let output = dir.join("missing.replay.json");
        assert!(replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).is_err());
        let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(result["status"], "draft");
        assert_eq!(result["pulls"][0]["replay"]["summary"]["missing"], 1);
        assert_eq!(result["pulls"][0]["replay"]["summary"]["censored"], 0);

        // An explicit branch skips the missing helper, e.g. A jumps directly to the observed C.
        let mut timeline: Value =
            serde_saphyr::from_str(&fs::read_to_string(&yaml).unwrap()).unwrap();
        timeline["entries"][2]["jump"] = serde_json::to_value(crate::timeline::Jump {
            to: crate::timeline::Destination::Time(0.4),
            when: crate::timeline::JumpWhen::Sync,
        })
        .unwrap();
        fs::write(&yaml, serde_saphyr::to_string(&timeline).unwrap()).unwrap();
        let output = dir.join("branch.replay.json");
        replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).unwrap();
        let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(result["status"], "internally_validated");
        assert_eq!(
            result["pulls"][0]["replay"]["rows"][1]["status"],
            "notVisited"
        );
        assert_eq!(result["pulls"][0]["replay"]["summary"]["missing"], 0);
        assert_eq!(result["pulls"][0]["replay"]["summary"]["matches"], 2);
    });
}

#[test]
fn replay_multi_pull_sync_and_disabled_rows_use_original_indices() {
    let a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90002, "cast"),
            (5500, 10, 90002, "cast"),
        ],
        6000,
        true,
    );
    let b = multi_log(
        "b",
        &[
            (1100, 10, 90001, "cast"),
            (5100, 10, 90002, "cast"),
            (5600, 10, 90002, "cast"),
        ],
        6100,
        true,
    );
    with_logs(&[a, b], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir, &yaml, GenerateMode::Raid).unwrap();
        let output = dir.join("result.replay.json");
        replay::replay_file(&yaml, dir, &output, None).unwrap();
        let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(result["pulls"].as_array().unwrap().len(), 2);
        for pull in result["pulls"].as_array().unwrap() {
            assert_eq!(pull["replay"]["rows"][1]["status"], "observedWithoutSync");
            assert_eq!(pull["replay"]["summary"]["matches"], 1);
            assert_eq!(pull["replay"]["resetClockMs"], 0);
        }
    });
}

#[rstest]
#[case::dungeon(GenerateMode::Dungeon)]
#[case::alliance(GenerateMode::Alliance)]
fn replay_boss_mode_holdout_uses_filtered_correspondence_and_all_raw_signals(
    #[case] mode: GenerateMode,
) {
    let rows = [
        (500, 11, 90003, "cast"),
        (1000, 10, 90001, "cast"),
        (2000, 11, 90003, "cast"),
        (3000, 10, 90002, "cast"),
        (5000, 11, 90003, "cast"),
    ];
    with_logs(
        &[
            multi_log("a", &rows, 6000, true),
            multi_log("b", &rows, 6000, true),
            multi_log("holdout", &rows, 6000, true),
        ],
        |dir| {
            let train = dir.join("train");
            fs::create_dir(&train).unwrap();
            for i in 0..2 {
                fs::copy(
                    dir.join(format!("fight_{i}.json")),
                    train.join(format!("fight_{i}.json")),
                )
                .unwrap();
            }
            let yaml = dir.join("dungeon.yaml");
            generate(&train, &yaml, mode).unwrap();
            let output = dir.join("dungeon.replay.json");
            replay::replay_file(&yaml, dir.join("fight_2.json"), &output, None).unwrap();
            let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            assert_eq!(result["validation"]["replay"], true);
            assert_eq!(
                result["pulls"][0]["replay"]["rows"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
            assert_eq!(
                result["pulls"][0]["replay"]["rows"][1]["status"],
                "observedWithoutSync"
            );
        },
    );
}

fn with_logs(logs: &[Value], check: impl FnOnce(&Path)) {
    let directory = tempfile::tempdir().unwrap();
    for (i, log) in logs.iter().enumerate() {
        fs::write(
            directory.path().join(format!("fight_{i}.json")),
            serde_json::to_vec(log).unwrap(),
        )
        .unwrap();
    }
    check(directory.path());
}

#[rstest]
#[case::raid("raid")]
#[case::dungeon("dungeon")]
#[case::alliance("alliance")]
fn parallel_prepare_preserves_output_order_and_bytes(#[case] mode: &str) {
    // Exercise the whole pipeline, e.g. repeat checks and converted text must agree with one worker.
    with_logs(
        &[
            repeat_log("clear", 2, true),
            repeat_log("wipe", 2, false),
            repeat_log("another-clear", 2, true),
        ],
        |dir| {
            let mut expected = None;
            for threads in [1, 4] {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build()
                    .unwrap();
                let output = dir.join("draft.yaml");
                let paths = [
                    output.clone(),
                    output.with_extension("report.json"),
                    output.with_extension("report.md"),
                    output.with_extension("replay.json"),
                    output.with_extension("replay.md"),
                    output.with_extension("txt"),
                ];
                let alignment = pool.install(|| {
                    let inputs: Vec<_> = (0..3)
                        .map(|i| dir.join(format!("fight_{i}.json")))
                        .collect();
                    let alignment = serde_json::to_vec(&align(&inputs).unwrap()).unwrap();
                    prepare(dir, &output, &["--mode", mode, "--convert"]).unwrap();
                    alignment
                });
                let bytes: Vec<_> = paths.iter().map(|path| fs::read(path).unwrap()).collect();
                let actual = (alignment, bytes);
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual);
                }
                for path in paths {
                    fs::remove_file(path).unwrap();
                }
            }
        },
    );
}

#[test]
fn replay_recognizes_aliased_training_paths() {
    // Cached path identity must retain direct evidence, e.g. logs/./fight.json names the training file.
    let rows = [
        (1000, 10, 91001, "cast"),
        (5000, 10, 91002, "cast"),
        (9000, 10, 91003, "cast"),
    ];
    with_logs(
        &[
            multi_log("first", &rows, 10000, true),
            multi_log("second", &rows, 10000, true),
        ],
        |dir| {
            let timeline = dir.join("draft.yaml");
            let output = dir.join("replay.json");
            generate(dir, &timeline, GenerateMode::Raid).unwrap();
            replay::replay_file(&timeline, dir.join("."), &output, None).unwrap();
            let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            for pull in report["pulls"].as_array().unwrap() {
                assert_eq!(pull["replay"]["passed"], true);
                assert_eq!(pull["replay"]["summary"]["matches"], 3);
            }
        },
    );
}

#[test]
fn parallel_inspection_preserves_first_input_error() {
    // A later parse failure cannot override an earlier duplicate, even if its worker finishes first.
    with_logs(&[sample()], |dir| {
        let valid = dir.join("fight_0.json");
        let missing = dir.join("missing.json");
        for threads in [1, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            pool.install(|| {
                let error = inspect(&[&valid, &valid, &missing]).unwrap_err();
                assert_eq!(error.to_string(), "Duplicate pull input");
                let error = inspect(&[&missing, &valid, &valid]).unwrap_err();
                assert!(error.to_string().contains("Invalid input"));
                assert!(error.to_string().contains("missing.json"));
            });
        }
    });
}

// Exercise CLI parsing and execution together, e.g. prepare must forward group selection to generation.
fn prepare(input: &Path, output: &Path, options: &[&str]) -> anyhow::Result<()> {
    use std::ffi::OsStr;

    use usage::Run;

    let mut args = vec![
        OsStr::new("prepare"),
        input.as_os_str(),
        OsStr::new("-o"),
        output.as_os_str(),
    ];
    args.extend(options.iter().map(OsStr::new));
    crate::cli::MainCli::parse_from(&args)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .command
        .run()
}

#[rstest]
#[case::single_default(None, false, false)]
#[case::multiple_raid(Some("raid"), true, false)]
#[case::multiple_dungeon(Some("dungeon"), true, false)]
#[case::multiple_alliance(Some("alliance"), true, false)]
#[case::single_converted(None, false, true)]
#[case::multiple_converted(Some("alliance"), true, true)]
fn prepare_generates_validated_yaml_and_replay_reports(
    #[case] mode: Option<&str>,
    #[case] multiple: bool,
    #[case] convert: bool,
) {
    let a = multi_log("a", &[(1000, 10, 90001, "cast")], 5000, true);
    let b = multi_log("b", &[(1100, 10, 90001, "cast")], 5000, false);
    let mut unrelated = multi_log("other", &[(1000, 10, 90001, "cast")], 5000, true);
    unrelated["report"]["fights"][0]["encounterID"] = json!(10000);
    with_logs(&[a, b, unrelated], |dir| {
        let input = if multiple {
            dir.to_path_buf()
        } else {
            dir.join("fight_0.json")
        };
        let output = dir.join("out/draft.yaml");
        let mut options = vec![
            "--name",
            "Unseen Fight",
            "--encounter",
            "9999",
            "--difficulty",
            "9",
            "--lookahead",
            "45",
        ];
        if let Some(mode) = mode {
            options.extend(["--mode", mode]);
        }
        if convert {
            options.push("--convert");
        }
        prepare(&input, &output, &options).unwrap();
        crate::timeline::validate_file(&output).unwrap();
        let timeline: Timeline =
            serde_saphyr::from_str(&fs::read_to_string(&output).unwrap()).unwrap();
        assert_eq!(
            timeline.reset_on,
            match mode {
                Some("dungeon") => vec![ResetEvent::AreaClear],
                Some("alliance") => vec![ResetEvent::Wipe, ResetEvent::AreaClear],
                _ => vec![ResetEvent::Wipe],
            }
        );
        for extension in ["report.json", "report.md", "replay.json", "replay.md"] {
            assert!(output.with_extension(extension).exists());
        }
        let result: Value =
            serde_json::from_slice(&fs::read(output.with_extension("replay.json")).unwrap())
                .unwrap();
        assert_eq!(result["status"], "internally_validated");
        assert_eq!(result["validation"]["replay"], true);
        assert_eq!(result["validation"]["runtime"], false);
        assert_eq!(
            result["pulls"].as_array().unwrap().len(),
            if multiple { 2 } else { 1 }
        );
        let text = output.with_extension("txt");
        if convert {
            assert_eq!(
                fs::read_to_string(text).unwrap(),
                crate::timeline::convert(&fs::read_to_string(&output).unwrap()).unwrap()
            );
        } else {
            assert!(!text.exists());
        }
    });
}

#[test]
fn prepare_preserves_existing_outputs_before_generating() {
    for extension in [
        "yaml",
        "report.json",
        "report.md",
        "replay.json",
        "replay.md",
    ] {
        with_file(&sample(), |input| {
            let output = input.with_file_name("draft.yaml");
            let existing = output.with_extension(extension);
            fs::write(&existing, "previous").unwrap();
            assert!(prepare(input, &output, &[]).is_err());
            assert_eq!(fs::read_to_string(&existing).unwrap(), "previous");
            assert_eq!(fs::read_dir(input.parent().unwrap()).unwrap().count(), 2);
        });
    }
}

#[rstest]
#[case::default(false)]
#[case::convert(true)]
fn prepare_preserves_existing_text(#[case] convert: bool) {
    with_file(&sample(), |input| {
        let output = input.with_file_name("draft.yaml");
        let text = output.with_extension("txt");
        fs::write(&text, "previous").unwrap();
        let result = prepare(input, &output, if convert { &["--convert"] } else { &[] });
        assert_eq!(result.is_err(), convert);
        assert_eq!(output.exists(), !convert);
        assert_eq!(fs::read_to_string(&text).unwrap(), "previous");
        if convert {
            assert_eq!(fs::read_dir(input.parent().unwrap()).unwrap().count(), 2);
        }
    });
}

#[test]
fn prepare_rejects_colliding_yaml_and_text_paths() {
    with_file(&sample(), |input| {
        let output = input.with_file_name("draft.txt");
        assert!(prepare(input, &output, &["--convert"]).is_err());
        assert_eq!(fs::read_dir(input.parent().unwrap()).unwrap().count(), 1);
    });
}

#[test]
fn prepare_rejects_invalid_input_without_outputs() {
    let mut invalid = sample();
    invalid["collection"]["complete"] = json!(false);
    with_file(&invalid, |input| {
        let output = input.with_file_name("draft.yaml");
        assert!(prepare(input, &output, &[]).is_err());
        assert_eq!(fs::read_dir(input.parent().unwrap()).unwrap().count(), 1);
    });
}

fn draft_events(yaml: &str) -> Vec<Value> {
    let timeline: Value = serde_saphyr::from_str(yaml).unwrap();
    timeline["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "event" && e["sync"]["log"] != "InCombat")
        .cloned()
        .collect()
}

// Exercise the consumer's stopped state, e.g. a 16s opening needs a separate combat-start sync.
#[test]
fn generated_draft_declares_combat_start_and_embedded_field_patterns() {
    with_file(&sample(), |input| {
        let output = input.with_extension("yaml");
        generate(input, &output, GenerateMode::Raid).unwrap();
        let yaml = fs::read_to_string(&output).unwrap();
        let timeline: Value = serde_saphyr::from_str(&yaml).unwrap();
        assert_eq!(timeline["entries"][0]["at"], 0.0);
        assert_eq!(timeline["entries"][0]["sync"]["log"], "InCombat");
        assert_eq!(
            timeline["entries"][0]["sync"]["fields"]["inGameCombat"],
            "1"
        );
        assert_eq!(timeline["entries"][0]["sync"]["window"], json!([0.0, 1.0]));
        let text = crate::timeline::convert(&yaml).unwrap();
        assert!(text.contains("0.0 \"--sync--\" InCombat { inGameCombat: \"1\" } window 0,1"));
        assert!(text.contains("Ability { id: \"15F91\", source: \"Boss\" }"));
        let replay_output = input.with_extension("replay.json");
        replay::replay_file(&output, input, &replay_output, None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(replay_output).unwrap()).unwrap();
        assert_eq!(report["pulls"][0]["replay"]["combatStartEntry"], 0);
        assert_eq!(report["pulls"][0]["replay"]["summary"]["matches"], 2);
    });
}

fn snapshot_draft(name: &str, yaml: &str, report: &Value, input: &Path) {
    // Preserve every provenance reference while removing the random root, e.g. /tmp/run/fight_0.json.
    let serialized = serde_json::to_string(report).unwrap();
    let escaped_path = serde_json::to_string(&slash_path(input)).unwrap();
    let stable: Value =
        serde_json::from_str(&serialized.replace(escaped_path.trim_matches('"'), "[input]"))
            .unwrap();
    insta::assert_snapshot!(format!("{name}_yaml"), yaml);
    insta::assert_json_snapshot!(format!("{name}_report"), stable);
    insta::assert_snapshot!(format!("{name}_markdown"), report::render(&stable).unwrap());
}

#[test]
fn multi_draft_merges_isolated_alternatives_and_keeps_block_timing_provenance() {
    let a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (1400, 11, 90002, "begincast"),
            (1500, 11, 90002, "cast"),
            (4000, 10, 90004, "cast"),
        ],
        6000,
        true,
    );
    let b = multi_log(
        "b",
        &[
            (2000, 10, 90001, "cast"),
            (2600, 11, 90003, "begincast"),
            (2701, 11, 90003, "cast"),
            (5000, 10, 90004, "cast"),
        ],
        6000,
        true,
    );
    with_logs(&[a, b], |path| {
        let selected = input::select_group(path, Some("Unseen Fight"), None, None).unwrap();
        let (yaml, report) = multi::build(selected, GenerateMode::Raid, 30.0).unwrap();
        snapshot_draft("multi", &yaml, &report, path);
        let events = draft_events(&yaml);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["at"], 1.5);
        assert_eq!(events[1]["at"], 2.1);
        assert_eq!(events[2]["at"], 4.5);
        assert_eq!(events[1]["sync"]["fields"]["id"], json!(["15F92", "15F93"]));
        assert_eq!(
            report["slots"][1]["time"],
            json!({"medianMs":600.5,"minMs":500,"maxMs":701,"sampleCount":2})
        );
        assert_eq!(
            report["blocks"][1]["time"],
            json!({"medianMs":1500.0,"minMs":1000,"maxMs":2000,"sampleCount":2})
        );
        assert_eq!(report["slots"][1]["samples"][1]["eventIndices"], json!([2]));
        assert_eq!(report["unobservedCombinations"], "unknown");
        assert_eq!(report["validation"]["replay"], false);
        assert!(report["omittedSignals"].as_array().unwrap().is_empty());
        let mut reversed = input::select_group(path, None, None, None).unwrap();
        reversed.pulls.reverse();
        let (reversed_yaml, reversed_report) =
            multi::build(reversed, GenerateMode::Raid, 30.0).unwrap();
        assert_eq!(yaml, reversed_yaml);
        assert_eq!(report, reversed_report);
        let output = path.join("out.yaml");
        generate(path, &output, GenerateMode::Raid).unwrap();
        assert_eq!(yaml, fs::read_to_string(&output).unwrap());
        let markdown = fs::read_to_string(output.with_extension("report.md")).unwrap();
        assert!(markdown.contains("600.5"));
        assert!(markdown.contains("| 원본 재생 | 미실행 |"));
        fs::remove_file(output.with_extension("report.md")).unwrap();
        markdown_file(output.with_extension("report.json")).unwrap();
        assert_eq!(
            markdown,
            fs::read_to_string(output.with_extension("report.md")).unwrap()
        );
        assert!(generate(path, &output, GenerateMode::Raid).is_err());
        let second_output = path.join("second.yaml");
        generate(path, &second_output, GenerateMode::Raid).unwrap();
        assert_eq!(yaml, fs::read_to_string(second_output).unwrap());
    });
}

#[rstest]
#[case::adjacent(false)]
#[case::shared_intermediate(true)]
fn dependent_paths_compile_discriminated_blocks_instead_of_independent_id_arrays(
    #[case] intermediate: bool,
) {
    let mut a = vec![(1000, 10, 90001, "cast"), (2000, 11, 90002, "cast")];
    let mut b = vec![(1000, 10, 90001, "cast"), (2000, 11, 90003, "cast")];
    if intermediate {
        a.push((2500, 11, 90007, "cast"));
        b.push((2500, 11, 90007, "cast"));
    }
    a.extend([(3000, 11, 90005, "cast"), (4000, 10, 90004, "cast")]);
    b.extend([(3000, 11, 90006, "cast"), (4000, 10, 90004, "cast")]);
    with_logs(
        &[
            multi_log("a", &a, 5000, true),
            multi_log("b", &b, 5000, true),
        ],
        |path| {
            let (yaml, report) = multi::build(
                input::select_group(path, None, None, None).unwrap(),
                GenerateMode::Raid,
                30.0,
            )
            .unwrap();
            let events = draft_events(&yaml);
            assert_eq!(events.len(), if intermediate { 9 } else { 7 });
            assert!(
                events
                    .iter()
                    .all(|event| event["sync"]["fields"]["id"].is_string())
            );
            assert_eq!(report["extensions"]["accepted"], true);
            assert_eq!(
                report["extensions"]["branches"].as_array().unwrap().len(),
                1
            );
            assert_eq!(report["validation"]["replay"], true);
            assert!(
                report["outputCoverage"][1]["omittedEventIndices"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        },
    );
}

#[test]
fn finite_repeats_keep_wipe_suffix_with_reached_sample_counts() {
    let a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90001, "cast"),
            (9000, 10, 90001, "cast"),
        ],
        10000,
        true,
    );
    let b = multi_log(
        "b",
        &[(1000, 10, 90001, "cast"), (5000, 10, 90001, "cast")],
        6000,
        false,
    );
    with_logs(&[a, b], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        let events = draft_events(&yaml);
        assert_eq!(
            events
                .iter()
                .map(|e| e["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [1.0, 5.0, 9.0]
        );
        assert_eq!(report["slots"][2]["time"]["sampleCount"], 1);
        assert_eq!(
            report["slots"][2]["unobservedAfterWipe"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(events.iter().all(|e| e.get("jump").is_none()));
    });
}

#[test]
fn three_pull_consensus_merges_only_the_same_position() {
    let logs = [90002, 90003, 90005]
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            multi_log(
                &format!("report{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (2000, 11, id, "cast"),
                    (4000, 10, 90004, "cast"),
                ],
                5000,
                true,
            )
        })
        .collect::<Vec<_>>();
    with_logs(&logs, |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(
            draft_events(&yaml)[1]["sync"]["fields"]["id"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert_eq!(report["slots"][1]["time"]["sampleCount"], 3);
        assert_eq!(report["slots"][1]["samples"].as_array().unwrap().len(), 3);
    });
}

#[test]
fn directory_selection_requires_one_group_and_never_connects_encounters() {
    let a = multi_log("a", &[(1000, 10, 90001, "cast")], 5000, true);
    let mut b = a.clone();
    b["report"]["code"] = json!("b");
    b["collection"]["reportCode"] = json!("b");
    b["report"]["fights"][0]["encounterID"] = json!(123456);
    b["report"]["fights"][0]["name"] = json!("Another New Encounter");
    with_logs(&[a, b.clone()], |path| {
        let error = input::select_group(path, None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--name") && error.contains("Another New Encounter"));
        input::select_group(path, Some("Missing"), None, None).unwrap_err();
        let selected = input::select_group(path, Some("Unseen Fight"), None, None).unwrap();
        assert_eq!(selected.pulls.len(), 1);
        assert_eq!(selected.key.encounter, 9999);
        let output = path.join("selected.yaml");
        draft::generate_selected(
            path,
            &output,
            GenerateMode::Raid,
            Some("Unseen Fight"),
            None,
            None,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&fs::read_to_string(output).unwrap()).len(), 1);
    });
    b["report"]["fights"][0]["name"] = json!("Unseen Fight");
    with_logs(
        &[multi_log("a", &[(1000, 10, 90001, "cast")], 5000, true), b],
        |path| {
            input::select_group(path, Some("Unseen Fight"), None, None).unwrap_err();
        },
    );
}

#[test]
fn directory_rejects_duplicate_and_incomplete_inputs_before_writing() {
    let a = multi_log("a", &[(1000, 10, 90001, "cast")], 5000, true);
    with_logs(&[a.clone(), a.clone()], |path| {
        let output = path.join("duplicate.yaml");
        assert!(generate(path, &output, GenerateMode::Raid).is_err());
        assert!(!output.exists());
    });
    let mut b = a.clone();
    b["collection"]["complete"] = json!(false);
    with_logs(&[a, b], |path| {
        let output = path.join("incomplete.yaml");
        assert!(generate(path, &output, GenerateMode::Raid).is_err());
        assert!(!output.exists());
    });
}

#[test]
fn multi_sync_checks_alternative_ids_against_excluded_raw_casts() {
    let a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (4000, 11, 90002, "cast"),
            (9000, 10, 90004, "cast"),
        ],
        10000,
        true,
    );
    let mut b = multi_log(
        "b",
        &[
            (1000, 10, 90001, "cast"),
            (4000, 11, 90003, "cast"),
            (9000, 10, 90004, "cast"),
        ],
        10000,
        true,
    );
    b["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"gameID":90002,"name":"Ability 90002","type":"1"}));
    b["events"].as_array_mut().unwrap().push(json!({"timestamp":5200,"type":"cast","sourceID":11,"abilityGameID":90002,"melee":true,"fight":2}));
    b["collection"]["eventCount"] = json!(4);
    with_logs(&[a, b], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml)[1]["sync"]["enabled"], false);
        assert_eq!(
            report["syncConflicts"][0]["conflictingEvents"][0]["eventIndex"],
            3
        );
    });
}

#[rstest]
#[case::dungeon(GenerateMode::Dungeon)]
#[case::alliance(GenerateMode::Alliance)]
fn multi_boss_mode_keeps_helper_only_inside_boss_spans(#[case] mode: GenerateMode) {
    let rows = [
        (100, 11, 90002, "cast"),
        (1000, 10, 90001, "cast"),
        (2000, 11, 90002, "cast"),
        (3000, 10, 90004, "cast"),
        (4000, 11, 90002, "cast"),
    ];
    with_logs(
        &[
            multi_log("a", &rows, 5000, true),
            multi_log("b", &rows, 5000, true),
        ],
        |path| {
            let (yaml, report) = multi::build(
                input::select_group(path, None, None, None).unwrap(),
                mode,
                30.0,
            )
            .unwrap();
            assert_eq!(draft_events(&yaml).len(), 3);
            assert_eq!(
                crate::timeline::convert(&yaml)
                    .unwrap()
                    .contains("ActorControl"),
                mode != GenerateMode::Dungeon
            );
            assert_eq!(
                crate::timeline::convert(&yaml)
                    .unwrap()
                    .contains("SystemLogMessage { id: \"7DE\" }"),
                mode != GenerateMode::Raid
            );
            assert_eq!(report["inputs"][0]["bossSegments"][0]["startMs"], 1000);
            assert_eq!(
                report["inputs"][0]["occurrences"].as_array().unwrap().len(),
                5
            );
            assert_eq!(
                report["observedPaths"][0]["occurrences"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
        },
    );
}

#[test]
fn censored_pull_raw_cast_still_disables_a_later_sync() {
    let a = multi_log(
        "a",
        &[(1000, 10, 90001, "cast"), (4000, 11, 90002, "cast")],
        5000,
        true,
    );
    let mut b = multi_log(
        "b",
        &[(1000, 10, 90001, "cast"), (2500, 11, 90002, "cast")],
        3000,
        false,
    );
    b["events"][1]["melee"] = json!(true);
    with_logs(&[a, b], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml).len(), 2);
        assert_eq!(draft_events(&yaml)[1]["sync"]["enabled"], false);
        assert_eq!(report["slots"][1]["time"]["sampleCount"], 1);
        assert_eq!(
            report["syncConflicts"][0]["conflictingEvents"][0]["eventIndex"],
            1
        );
    });
}

#[test]
fn simultaneous_instances_are_preserved_without_inflating_pull_sample_counts() {
    let mut a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (3000, 11, 90002, "cast"),
            (3000, 11, 90002, "cast"),
            (7000, 10, 90004, "cast"),
        ],
        8000,
        true,
    );
    a["events"][1]["sourceInstance"] = json!(2);
    a["events"][2]["sourceInstance"] = json!(3);
    let mut b = a.clone();
    b["report"]["code"] = json!("b");
    b["collection"]["reportCode"] = json!("b");
    with_logs(&[a, b], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml).len(), 3);
        assert_eq!(report["slots"][1]["time"]["sampleCount"], 2);
        assert_eq!(
            report["slots"][1]["samples"][0]["eventIndices"],
            json!([1, 2])
        );
        assert_eq!(
            report["slots"][1]["samples"][0]["instanceIds"],
            json!([2, 3])
        );
        assert_eq!(draft_events(&yaml)[1]["sync"]["enabled"], false);
    });
}

#[test]
fn boss_block_entries_use_observed_medians_without_accumulating_interval_medians() {
    let logs = [
        [1000, 9000, 10000],
        [2000, 3000, 13000],
        [4000, 8000, 12000],
    ]
    .iter()
    .enumerate()
    .map(|(i, times)| {
        multi_log(
            &format!("report{i}"),
            &[
                (times[0], 10, 90001, "cast"),
                (times[1], 10, 90002, "cast"),
                (times[2], 10, 90003, "cast"),
            ],
            14000,
            true,
        )
    })
    .collect::<Vec<_>>();
    with_logs(&logs, |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(
            draft_events(&yaml)
                .iter()
                .map(|e| e["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [2.0, 6.0, 60.7]
        );
        assert_eq!(report["alignmentSlots"][1]["time"]["medianMs"], 4000.0);
        assert_eq!(report["alignmentBlocks"][2]["time"]["medianMs"], 8000.0);
        assert_eq!(report["alignmentBlocks"][3]["time"]["medianMs"], 12000.0);
        assert_eq!(report["slots"][1]["clockTime"]["medianMs"], 6000.0);
        assert_eq!(report["extensions"]["accepted"], true);
    });
}

#[test]
fn mixed_roles_preserve_common_successors_when_interval_medians_differ() {
    // Every pull has A → helper → B; mixing absolute and interval medians must not drop B.
    let logs = [
        (1000, 11000, 12000),
        (9000, 10000, 11000),
        (10000, 20000, 21000),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (a, helper, b))| {
        multi_log(
            &format!("report{i}"),
            &[
                (a, 10, 90001, "cast"),
                (helper, 11, 90002, "cast"),
                (b, 10, 90003, "cast"),
            ],
            25000,
            true,
        )
    })
    .collect::<Vec<_>>();
    with_logs(&logs, |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(
            draft_events(&yaml)
                .iter()
                .map(|event| event["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [9.0, 79.1, 140.2]
        );
        assert_eq!(report["slots"][1]["time"]["medianMs"], 10000.0);
        assert_eq!(report["extensions"]["accepted"], true);
        assert!(report["omittedSignals"].as_array().unwrap().is_empty());
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["representedEventIndices"], json!([0, 1, 2]));
            assert!(
                coverage["omittedEventIndices"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    });
}

#[rstest]
#[case::boss_prefix(10, false)]
#[case::boss_after_divergent_paths(10, true)]
#[case::helper_prefix(11, false)]
#[case::helper_after_divergent_paths(11, true)]
fn observed_common_casts_survive_wipe_when_pull_timings_drift(
    #[case] actor: i64,
    #[case] divergent: bool,
) {
    let mut a = vec![(1000, 10, 90001, "cast")];
    let mut b = a.clone();
    if divergent {
        // An observed common C restores the timing reference after X/Y before the slower B.
        a.extend([(2000, 10, 90004, "cast"), (10000, 10, 90006, "cast")]);
        b.extend([(2000, 10, 90005, "cast"), (12000, 10, 90006, "cast")]);
    }
    a.extend([(25000, actor, 90002, "cast"), (40000, 10, 90003, "cast")]);
    b.push((18000, actor, 90002, "cast"));
    with_logs(
        &[
            multi_log("a", &a, 50000, true),
            multi_log("b", &b, 20000, false),
        ],
        |path| {
            let group = input::select_group(path, None, None, None).unwrap();
            let comparison = alignment::compare(&group.pulls[0], &group.pulls[1]).unwrap();
            assert!(
                comparison
                    .segments
                    .iter()
                    .flat_map(|segment| &segment.slots)
                    .any(|slot| {
                        matches!((&slot.left, &slot.right), (Some(a), Some(b))
                if a.key.ability_id == 90002 && b.key.ability_id == 90002)
                    })
            );
            let (_, report) = multi::build(group, GenerateMode::Raid, 30.0).unwrap();
            let common = report["slots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|slot| slot["abilityIds"] == json!([90002]))
                .unwrap();
            assert_eq!(
                common["absoluteTime"],
                json!({
                    "medianMs":21500.0,"minMs":18000,"maxMs":25000,"sampleCount":2
                })
            );
            assert!(common["unobservedAfterWipe"].as_array().unwrap().is_empty());
            assert!(
                report["outputCoverage"][1]["omittedEventIndices"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let tail = report["slots"]
                .as_array()
                .unwrap()
                .iter()
                .find(|slot| slot["abilityIds"] == json!([90003]))
                .unwrap();
            assert_eq!(tail["time"]["sampleCount"], 1);
            assert_eq!(tail["unobservedAfterWipe"].as_array().unwrap().len(), 1);
        },
    );
}

fn sample() -> Value {
    json!({
        "report": {"code":"new-report", "revision":1,"startTime":0,"endTime":3000,
            "masterData":{"lang":"en","gameVersion":1,"logVersion":76,
                "actors":[{"id":10,"name":"Boss", "gameID":99901,"type":"NPC","subType":"Boss"},
                          {"id":11,"name":"Helper", "gameID":99902,"type":"NPC","subType":"NPC"}],
                "abilities":[{"gameID":90001,"name":"Move","type":"1"}]},
            "fights":[{"id":2,"name":"Unseen Fight","encounterID":9999,"difficulty":9,
                "startTime":1000,"endTime":2000,"inProgress":false,"kill":true,
                "enemyNPCs":[{"id":10,"gameID":99901},{"id":11,"gameID":99902}],"enemyPets":[]}]},
        "collection":{"schemaVersion":1,"toolVersion":"0.1.0","collectedAtUnixMs":1,
            "requests":{},"complete":true,"nextPageTimestamp":null,"reportCode":"new-report",
            "fightID":2,"startTime":1000,"endTime":2000,"eventCount":4,
            "pageCount":1,"pageStartTimes":[1000]},
        "events":[
            {"timestamp":1500,"type":"cast","sourceID":11,"sourceInstance":2,"abilityGameID":90001,"fight":2},
            {"timestamp":1100,"type":"begincast","sourceID":10,"abilityGameID":90001,"fight":2},
            {"timestamp":1500,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2},
            {"timestamp":1900,"type":"begincast","sourceID":10,"abilityGameID":90001,"fight":2}
        ]
    })
}

fn with_file(data: &Value, check: impl FnOnce(&Path)) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fight.json");
    fs::write(&path, serde_json::to_vec(data).unwrap()).unwrap();
    check(&path);
}

#[test]
fn slash_paths_preserve_native_path_characters() {
    #[cfg(not(windows))]
    for path in ["logs/fight.json", r"logs\literal/fight.json"] {
        // A Unix backslash is part of a filename, e.g. logs\literal is one directory.
        assert_eq!(slash_path(Path::new(path)), path);
    }
    #[cfg(windows)]
    for (native, expected) in [
        (r"logs\fight.json", "logs/fight.json"),
        (r"C:\logs\fight.json", "C:/logs/fight.json"),
        (r"\\server\share\fight.json", "//server/share/fight.json"),
        (r"\\?\C:\logs\fight.json", "//?/C:/logs/fight.json"),
        (
            r"\\?\UNC\server\share\fight.json",
            "//?/UNC/server/share/fight.json",
        ),
    ] {
        // Keep prefixes reopenable, e.g. //?/C:/logs/fight.json returns to an extended Windows path.
        assert_eq!(slash_path(Path::new(native)), expected);
        assert_eq!(std::path::PathBuf::from_slash(expected), Path::new(native));
    }
}

#[test]
fn reports_use_slash_paths_and_generation_can_reopen_them() {
    with_file(&sample(), |input| {
        let expected = format!("{}/fight.json", slash_path(input.parent().unwrap()));
        let source = load_one(input).unwrap();
        assert_eq!(source.pull.file, expected);
        assert_eq!(std::path::PathBuf::from_slash(&source.pull.file), input);

        // Exercise the reload and persisted provenance together, e.g. Windows logs\fight.json.
        let output = input.with_extension("yaml");
        generate(input, &output, GenerateMode::Raid).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["input"]["file"], expected);
        assert!(
            fs::read_to_string(output.with_extension("report.md"))
                .unwrap()
                .contains(&expected)
        );
    });
}

#[test]
fn multi_draft_keeps_recollected_pull_and_raw_evidence_together() {
    let rows = [(1000, 10, 90001, "cast")];
    with_logs(
        &[
            multi_log("a", &rows, 5000, true),
            multi_log("b", &rows, 5000, true),
        ],
        |path| {
            let group = input::select_group(path, None, None, None).unwrap();
            // Recollection moves the common cast; normalization, medians and hashes must share the new read.
            let file = path.join("fight_1.json");
            let mut updated = multi_log(
                "b",
                &[(2000, 10, 90001, "cast"), (2200, 10, 90001, "cast")],
                5000,
                true,
            );
            // The excluded melee cast must still disable a sync from the same recollected raw log.
            updated["events"][1]["melee"] = json!(true);
            let bytes = serde_json::to_vec(&updated).unwrap();
            fs::write(&file, &bytes).unwrap();

            let (yaml, report) = multi::build(group, GenerateMode::Raid, 30.0).unwrap();
            assert_eq!(draft_events(&yaml)[0]["at"], 1.5);
            assert_eq!(report["inputs"][1]["input"]["sha256"], sha256(&bytes));
            assert_eq!(report["slots"][0]["samples"][1]["timeMs"], 2000);
            assert_eq!(draft_events(&yaml)[0]["sync"]["enabled"], false);
            assert_eq!(
                report["syncConflicts"][0]["conflictingEvents"],
                json!([{"file":slash_path(&file), "eventIndex":1}])
            );
            assert_eq!(
                report["observedPaths"][1]["occurrences"],
                report["inputs"][1]["occurrences"]
            );
            assert_eq!(
                report["observedPaths"][1]["occurrences"][0]["relative_ms"],
                2000
            );
        },
    );
}

#[test]
fn single_draft_uses_loaded_source_and_validates_before_serialization() {
    with_file(&sample(), |path| {
        let mut source = load_one(path).unwrap();
        fs::remove_file(path).unwrap();
        let draft = draft::build_single(&source, GenerateMode::Raid).unwrap();
        assert_eq!(
            draft_events(&draft::serialize_draft(draft.entries, vec![ResetEvent::Wipe]).unwrap())
                .len(),
            2
        );

        // Keep per-input validation even if a later merge could omit this ability's cast.
        source
            .log
            .report
            .master_data
            .abilities
            .first_mut()
            .unwrap()
            .name = "Bad\"Name".into();
        draft::build_single(&source, GenerateMode::Raid).unwrap_err();
    });
}

#[test]
fn stable_order_pairs_casts_and_keeps_unfinished_start() {
    with_file(&sample(), |path| {
        let groups = inspect(&[path]).unwrap();
        let pull = &groups[0].pulls[0];
        assert_eq!(pull.name, "Unseen Fight");
        assert!(pull.kill);
        let rows = &pull.occurrences;
        assert_eq!(
            rows.iter().map(|r| r.event_index).collect::<Vec<_>>(),
            [1, 0, 2, 3]
        );
        assert_eq!(rows[1].simultaneous, rows[2].simultaneous);
        assert_ne!(rows[0].simultaneous, rows[1].simultaneous);
        assert_eq!(rows[2].start_event_index, Some(1));
        assert_eq!(rows[2].start_timestamp_ms, Some(1100));
        assert_eq!(rows[0].completion_event_index, Some(2));
        assert_eq!(rows[3].completion_event_index, None);
        assert_eq!(rows[1].start_event_index, None);
        assert_eq!(rows[3].kind, "begincast");
        assert_eq!(rows[3].relative_ms, 900);
    });
}

#[rstest]
#[case::incomplete(|v: &mut Value| v["collection"]["complete"] = json!(false))]
#[case::event_count(|v: &mut Value| v["collection"]["eventCount"] = json!(3))]
#[case::timestamp(|v: &mut Value| v["events"][0]["timestamp"] = json!(2001))]
#[case::ability(|v: &mut Value| v["events"][0]["abilityGameID"] = json!(42))]
#[case::fight(|v: &mut Value| v["events"][0]["fight"] = json!(3))]
fn rejects_incomplete_and_invalid_references(#[case] mutate: fn(&mut Value)) {
    let mut data = sample();
    mutate(&mut data);
    with_file(&data, |path| {
        inspect(&[path]).unwrap_err();
    });
}

#[rstest]
#[case::parser_version(1, 77)]
#[case::game_version(2, 76)]
#[case::both_versions(2, 77)]
fn same_encounter_accepts_different_versions(#[case] game_version: i64, #[case] log_version: i64) {
    let mut changed = sample();
    changed["report"]["code"] = json!("other-report");
    changed["collection"]["reportCode"] = json!("other-report");
    changed["report"]["masterData"]["gameVersion"] = json!(game_version);
    changed["report"]["masterData"]["logVersion"] = json!(log_version);
    with_file(&sample(), |first| {
        with_file(&changed, |second| {
            let groups = inspect(&[first, second]).unwrap();
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].pulls.len(), 2);
            assert_eq!(groups[0].pulls[1].game_version, game_version);
            assert_eq!(groups[0].pulls[1].log_version, log_version);
            let alignment = serde_json::to_value(align(&[first, second]).unwrap()).unwrap();
            assert_eq!(alignment["inputs"][1]["gameVersion"], game_version);
            assert_eq!(alignment["inputs"][1]["logVersion"], log_version);
        })
    });
}

#[test]
fn directory_generation_accepts_mixed_versions_and_preserves_each_input_version() {
    let a = multi_log(
        "a",
        &[(1000, 10, 90001, "cast"), (3000, 10, 90002, "cast")],
        4000,
        true,
    );
    let mut b = a.clone();
    b["report"]["code"] = json!("b");
    b["collection"]["reportCode"] = json!("b");
    b["report"]["masterData"]["gameVersion"] = json!(2);
    b["report"]["masterData"]["logVersion"] = json!(74);
    with_logs(&[a, b], |path| {
        let output = path.join("mixed.yaml");
        generate(path, &output, GenerateMode::Raid).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["inputs"].as_array().unwrap().len(), 2);
        assert_eq!(report["inputs"][0]["input"]["gameVersion"], 1);
        assert_eq!(report["inputs"][0]["input"]["logVersion"], 76);
        assert_eq!(report["inputs"][1]["input"]["gameVersion"], 2);
        assert_eq!(report["inputs"][1]["input"]["logVersion"], 74);
        assert_eq!(report["group"], json!({"encounter":9999,"difficulty":9}));
        assert_eq!(report["slots"][0]["time"]["sampleCount"], 2);
    });
}

#[test]
fn wipe_on_an_early_branch_preserves_later_common_rows_without_matching_a_later_repeat() {
    // The short pull's B at 2s must not align with B at 20s on the other path.
    // Both surviving paths reach C at 5s, even though their early choices differ.
    let logs = [
        multi_log(
            "a",
            &[
                (1000, 10, 90001, "cast"),
                (2000, 10, 90002, "cast"),
                (5000, 10, 90004, "cast"),
                (20000, 10, 90003, "cast"),
                (25000, 10, 90005, "cast"),
            ],
            30000,
            true,
        ),
        multi_log(
            "b",
            &[
                (1000, 10, 90001, "cast"),
                (2000, 10, 90003, "cast"),
                (5000, 10, 90004, "cast"),
                (20000, 10, 90003, "cast"),
                (25000, 10, 90005, "cast"),
            ],
            30000,
            true,
        ),
        multi_log(
            "c",
            &[(1000, 10, 90001, "cast"), (2000, 10, 90003, "cast")],
            3000,
            false,
        ),
    ];
    with_logs(&logs, |path| {
        let group = input::select_group(path, None, None, None).unwrap();
        let comparison = alignment::compare(&group.pulls[0], &group.pulls[2]).unwrap();
        assert!(comparison.segments.iter().flat_map(|s| &s.slots).all(|s| {
            !matches!((&s.left,&s.right), (Some(a),Some(b)) if a.time_ms == 20000 && b.time_ms == 2000)
        }));
        let (yaml, report) = multi::build(group, GenerateMode::Raid, 30.0).unwrap();
        let events = draft_events(&yaml);
        assert!(
            events
                .iter()
                .any(|e| e["at"] == 5.0 && e["sync"]["fields"]["id"] == "15F94")
        );
        let common = report["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["abilityIds"] == json!([90004]))
            .unwrap();
        assert_eq!(common["time"]["sampleCount"], 2);
        assert_eq!(common["unobservedAfterWipe"].as_array().unwrap().len(), 1);
    });
}

#[test]
fn an_initial_wipe_does_not_remove_the_entire_multi_pull_draft() {
    let a = multi_log(
        "a",
        &[
            (10000, 10, 90001, "begincast"),
            (15000, 10, 90001, "cast"),
            (30000, 10, 90002, "cast"),
        ],
        40000,
        true,
    );
    let b = multi_log("b", &[(19000, 10, 90001, "begincast")], 20000, false);
    with_logs(&[a, b], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml).len(), 2);
        assert_eq!(report["slots"][0]["time"]["sampleCount"], 1);
        assert_eq!(
            report["slots"][0]["unobservedAfterWipe"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(report["omittedSignals"].as_array().unwrap().is_empty());
    });
}

#[test]
fn encounter_and_difficulty_selectors_resolve_overlapping_names() {
    let a = multi_log("a", &[(1000, 10, 90001, "cast")], 2000, true);
    let mut b = a.clone();
    b["report"]["code"] = json!("b");
    b["collection"]["reportCode"] = json!("b");
    b["report"]["fights"][0]["encounterID"] = json!(123456);
    let mut c = b.clone();
    c["report"]["code"] = json!("c");
    c["collection"]["reportCode"] = json!("c");
    c["report"]["fights"][0]["difficulty"] = json!(10);
    with_logs(&[a, b, c], |path| {
        let error = input::select_group(path, None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--encounter 9999 --difficulty 9"));
        assert!(error.contains("--encounter 123456 --difficulty 10"));
        input::select_group(path, Some("Unseen Fight"), None, None).unwrap_err();
        input::select_group(path, None, Some(123456), None).unwrap_err();
        let selected = input::select_group(path, None, Some(123456), Some(10)).unwrap();
        assert_eq!(selected.key.encounter, 123456);
        assert_eq!(selected.key.difficulty, 10);
        assert_eq!(selected.pulls.len(), 1);
        input::select_group(path, Some("Missing"), Some(123456), Some(10)).unwrap_err();
        let output = path.join("selected.yaml");
        draft::generate_selected(
            path,
            &output,
            GenerateMode::Raid,
            None,
            Some(123456),
            Some(10),
            30.0,
        )
        .unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["group"], json!({"encounter":123456,"difficulty":10}));
    });
}

#[test]
fn actor_role_conflicts_are_rejected_even_when_versions_differ() {
    let mut changed = sample();
    changed["report"]["code"] = json!("other-report");
    changed["collection"]["reportCode"] = json!("other-report");
    changed["report"]["masterData"]["gameVersion"] = json!(2);
    changed["report"]["masterData"]["logVersion"] = json!(74);
    changed["report"]["masterData"]["actors"][0]["subType"] = json!("NPC");
    with_file(&sample(), |first| {
        with_file(&changed, |second| {
            assert!(
                inspect(&[first, second])
                    .unwrap_err()
                    .to_string()
                    .contains("Actor role conflict")
            );
        })
    });
}

#[test]
fn accepts_same_ability_id_with_different_types_across_reports() {
    let mut changed = sample();
    changed["report"]["code"] = json!("other-report");
    changed["collection"]["reportCode"] = json!("other-report");
    changed["report"]["masterData"]["abilities"][0]["type"] = json!("128");
    with_file(&sample(), |first| {
        with_file(&changed, |second| {
            assert_eq!(inspect(&[first, second]).unwrap()[0].pulls.len(), 2);
        })
    });
}

#[test]
fn typed_log_preserves_variable_fields() {
    let mut raw = sample();
    raw["report"]["phases"] = json!([{"encounterID":9999,"phases":[{"id":1,"name":"Late"}]}]);
    raw["events"][0]["sourceResources"] = json!({"hitPoints":42,"x":1});
    raw["collection"]["requests"] = json!({"events":{"variables":{"fightIDs":[2]}}});
    let typed: CollectedLog = serde_json::from_value(raw.clone()).unwrap();
    let saved = serde_json::to_value(typed).unwrap();
    assert_eq!(saved["report"]["phases"], raw["report"]["phases"]);
    assert_eq!(
        saved["events"][0]["sourceResources"],
        raw["events"][0]["sourceResources"]
    );
    assert_eq!(
        saved["collection"]["requests"],
        raw["collection"]["requests"]
    );
}

#[rstest]
#[case::schema_version(|v: &mut Value| v["collection"]["schemaVersion"] = json!(2))]
#[case::negative_start(|v: &mut Value| v["collection"]["startTime"] = json!(-1))]
#[case::end_before_start(|v: &mut Value| v["collection"]["endTime"] = json!(999))]
#[case::missing_page_start(|v: &mut Value| v["collection"]["pageStartTimes"] = json!([]))]
#[case::late_page_start(|v: &mut Value| v["collection"]["pageStartTimes"] = json!([1000, 3000]))]
#[case::unexpected_next_page(|v: &mut Value| v["collection"]["nextPageTimestamp"] = json!(1500))]
#[case::page_count(|v: &mut Value| v["collection"]["pageCount"] = json!(2))]
#[case::duplicate_page_start(|v: &mut Value| v["collection"]["pageStartTimes"] = json!([1000, 1000]))]
#[case::report_code(|v: &mut Value| v["collection"]["reportCode"] = json!("wrong"))]
#[case::event_count(|v: &mut Value| v["collection"]["eventCount"] = json!(3))]
fn garde_rejects_invalid_collection_manifest(#[case] mutate: fn(&mut Value)) {
    use garde::Validate;

    let mut raw = sample();
    mutate(&mut raw);
    let typed: CollectedLog = serde_json::from_value(raw).unwrap();
    assert!(typed.validate().is_err());
}

#[test]
fn includes_enemy_players_but_excludes_melee_casts() {
    let mut data = sample();
    data["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(
            json!({"id":12,"name":"Opponent","gameID":99903,"type":"Player","subType":"Warrior"}),
        );
    data["report"]["fights"][0]["enemyPlayers"] = json!([12]);
    data["events"].as_array_mut().unwrap().extend([
        json!({"timestamp":1600,"type":"cast","sourceID":12,"abilityGameID":90001,"fight":2}),
        json!({"timestamp":1700,"type":"cast","sourceID":10,"abilityGameID":90001,"melee":true,"fight":2}),
    ]);
    data["collection"]["eventCount"] = json!(6);
    with_file(&data, |path| {
        let rows = &inspect(&[path]).unwrap()[0].pulls[0].occurrences;
        assert!(
            rows.iter()
                .any(|row| row.event_index == 4 && row.actor_id == 12)
        );
        assert!(!rows.iter().any(|row| row.event_index == 5));
    });
}

#[test]
fn canceled_start_stays_unfinished_after_same_ability_restarts() {
    let mut data = sample();
    data["events"] = json!([
        {"timestamp":1100,"type":"begincast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":1200,"type":"begincast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":1300,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":1400,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2}
    ]);
    with_file(&data, |path| {
        let rows = &inspect(&[path]).unwrap()[0].pulls[0].occurrences;
        assert_eq!(rows[0].completion_event_index, None);
        assert_eq!(rows[1].completion_event_index, Some(2));
        assert_eq!(rows[2].start_event_index, Some(1));
        assert_eq!(rows[3].start_event_index, None);
    });
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn occurrences_keep_stable_timestamp_order(
        first in 1000i64..=2000,
        second in 1000i64..=2000,
        third in 1000i64..=2000,
        fourth in 1000i64..=2000,
    ) {
        let times = [first, second, third, fourth];
        let mut data = sample();
        for (event, at) in data["events"].as_array_mut().unwrap().iter_mut().zip(times) {
            event["timestamp"] = json!(at);
        }
        let mut expected = [0, 1, 2, 3];
        expected.sort_by_key(|&index| times[index]);
        with_file(&data, |path| {
            let groups = inspect(&[path]).unwrap();
            let rows = &groups[0].pulls[0].occurrences;
            assert_eq!(
                rows.iter().map(|row| row.event_index).collect::<Vec<_>>(),
                expected
            );
            for row in rows {
                assert_eq!(row.timestamp_ms, times[row.event_index]);
                assert_eq!(row.relative_ms, times[row.event_index] - 1000);
            }
        });
    }
}

#[test]
fn generates_deterministic_draft_and_disables_excluded_cast_collision() {
    let mut data = sample();
    data["report"]["masterData"]["actors"][1]["name"] = json!("Helper (A)+");
    data["events"][0]["timestamp"] = json!(1150);
    data["events"].as_array_mut().unwrap().push(json!({
        "timestamp":1160,"type":"cast","sourceID":11,"sourceInstance":2,
        "abilityGameID":90001,"melee":true,"fight":2
    }));
    data["events"].as_array_mut().unwrap().push(json!({
        "timestamp":1150,"type":"cast","sourceID":11,"sourceInstance":3,
        "abilityGameID":90001,"fight":2
    }));
    data["collection"]["eventCount"] = json!(6);
    with_file(&data, |input| {
        let first = input.with_extension("first.yaml");
        let second = input.with_extension("second.yaml");
        generate(input, &first, GenerateMode::Raid).unwrap();
        generate(input, &second, GenerateMode::Raid).unwrap();
        let yaml = fs::read_to_string(&first).unwrap();
        assert_eq!(yaml, fs::read_to_string(&second).unwrap());
        assert!(yaml.starts_with("# yaml-language-server: $schema="));
        let parsed: Value = serde_saphyr::from_str(&yaml).unwrap();
        let events: Vec<&Value> = parsed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == "event" && entry["sync"]["log"] != "InCombat")
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["at"], json!(0.2));
        assert_eq!(
            events[0]["sync"]["fields"]["source"],
            json!(r"Helper \(A\)\+")
        );
        assert_eq!(events[0]["sync"]["enabled"], json!(false));
        assert_eq!(events[1]["at"], json!(0.5));
        let markdown = fs::read_to_string(first.with_extension("report.md")).unwrap();
        assert!(markdown.contains("| 표시할 완료 cast 행 | 2 |"));
        assert!(markdown.contains("| 대표 행에 묶은 동시 cast | 1 |"));
        assert!(markdown.contains("| sync 비활성화 | 1 |"));
        let second_markdown = second.with_extension("report.md");
        fs::remove_file(&second_markdown).unwrap();
        markdown_file(second.with_extension("report.json")).unwrap();
        assert_eq!(markdown, fs::read_to_string(&second_markdown).unwrap());
        assert!(markdown_file(first.with_extension("report.json")).is_err());
        let report: Value =
            serde_json::from_slice(&fs::read(first.with_extension("report.json")).unwrap())
                .unwrap();
        snapshot_draft("single", &yaml, &report, input);
        assert_eq!(
            report["syncConflicts"][0]["conflictingEventIndices"],
            json!([4, 5])
        );
        assert_eq!(report["collapsedCasts"][0]["representativeEventIndex"], 0);
        assert_eq!(
            report["collapsedCasts"][0]["omittedEventIndices"],
            json!([5])
        );
        assert_eq!(report["validation"]["cactbotParser"], false);
        assert!(generate(input, &first, GenerateMode::Raid).is_err());
        let third = input.with_extension("third.yaml");
        let existing_report = third.with_extension("report.json");
        fs::write(&existing_report, "keep").unwrap();
        assert!(generate(input, &third, GenerateMode::Raid).is_err());
        assert_eq!(fs::read_to_string(&existing_report).unwrap(), "keep");
        assert!(!third.exists());
        fs::remove_file(existing_report).unwrap();
        let fourth = input.with_extension("fourth.yaml");
        let existing_markdown = fourth.with_extension("report.md");
        fs::write(&existing_markdown, "keep").unwrap();
        assert!(generate(input, &fourth, GenerateMode::Raid).is_err());
        assert_eq!(fs::read_to_string(&existing_markdown).unwrap(), "keep");
        assert!(!fourth.exists());
        fs::remove_file(existing_markdown).unwrap();
        for path in [
            &first,
            &second,
            &first.with_extension("report.json"),
            &second.with_extension("report.json"),
            &first.with_extension("report.md"),
            &second.with_extension("report.md"),
        ] {
            fs::remove_file(path).unwrap();
        }
    });
}

#[test]
fn sync_conflicts_use_the_displayed_rounded_time() {
    let mut data = sample();
    data["report"]["endTime"] = json!(9000);
    data["report"]["fights"][0]["endTime"] = json!(9000);
    data["collection"]["endTime"] = json!(9000);
    data["collection"]["eventCount"] = json!(2);
    data["events"] = json!([
        {"timestamp":6050,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":8600,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2}
    ]);
    with_file(&data, |input| {
        let output = input.with_extension("rounded.yaml");
        generate(input, &output, GenerateMode::Raid).unwrap();
        let yaml = fs::read_to_string(&output).unwrap();
        let parsed: Value = serde_saphyr::from_str(&yaml).unwrap();
        let events: Vec<&Value> = parsed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == "event" && entry["sync"]["log"] != "InCombat")
            .collect();
        assert_eq!(events[0]["at"], json!(5.1));
        assert_eq!(events[0]["sync"]["enabled"], false);
        assert_eq!(events[1]["at"], json!(7.6));
        assert!(events[1]["sync"].get("enabled").is_none());
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["syncConflicts"][0]["conflictingEventIndices"],
            json!([1])
        );
        for path in [
            &output,
            &output.with_extension("report.json"),
            &output.with_extension("report.md"),
        ] {
            fs::remove_file(path).unwrap();
        }
    });
}

#[test]
fn dungeon_keeps_mechanics_inside_boss_segments() {
    let mut data = sample();
    data["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
    data["report"]["fights"][0]["enemyNPCs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"gameID":99903}));
    data["events"] = json!([
        {"timestamp":1050,"type":"cast","sourceID":11,"abilityGameID":90001,"fight":2},
        {"timestamp":1200,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":1300,"type":"cast","sourceID":11,"abilityGameID":90001,"fight":2},
        {"timestamp":1400,"type":"death","targetID":10,"fight":2},
        {"timestamp":1500,"type":"cast","sourceID":11,"abilityGameID":90001,"fight":2},
        {"timestamp":1700,"type":"cast","sourceID":12,"abilityGameID":90001,"fight":2},
        {"timestamp":1750,"type":"cast","sourceID":11,"abilityGameID":90001,"fight":2},
        {"timestamp":1800,"type":"death","targetID":12,"fight":2},
        {"timestamp":1900,"type":"cast","sourceID":11,"abilityGameID":90001,"fight":2}
    ]);
    data["collection"]["eventCount"] = json!(9);
    let mut without_boss = data.clone();
    for actor in without_boss["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
    {
        actor["subType"] = json!("NPC");
    }
    with_file(&without_boss, |input| {
        let output = input.with_extension("no-boss.yaml");
        for mode in [GenerateMode::Dungeon, GenerateMode::Alliance] {
            assert!(
                generate(input, &output, mode)
                    .unwrap_err()
                    .to_string()
                    .contains("No observed boss segment")
            );
            assert!(!output.exists());
        }
    });
    with_file(&data, |input| {
        for (mode, expected) in [
            (GenerateMode::Raid, vec![0, 1, 2, 4, 5, 6, 8]),
            (GenerateMode::Dungeon, vec![1, 2, 5, 6]),
            (GenerateMode::Alliance, vec![1, 2, 5, 6]),
        ] {
            let output = input.with_extension(if mode == GenerateMode::Dungeon {
                "dungeon.yaml"
            } else {
                "raid.yaml"
            });
            generate(input, &output, mode).unwrap();
            let yaml = fs::read_to_string(&output).unwrap();
            assert_eq!(
                crate::timeline::convert(&yaml)
                    .unwrap()
                    .contains("SystemLogMessage { id: \"7DE\" }"),
                mode != GenerateMode::Raid
            );
            assert_eq!(
                crate::timeline::convert(&yaml)
                    .unwrap()
                    .contains("ActorControl"),
                mode != GenerateMode::Dungeon
            );
            let report: Value =
                serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                    .unwrap();
            assert_eq!(report["mode"], json!(mode));
            assert_eq!(report["emittedEventIndices"], json!(expected));
            assert_eq!(
                report["bossSegments"],
                json!([
                    {"actorId":10,"startMs":200,"endMs":400},
                    {"actorId":12,"startMs":700,"endMs":800}
                ])
            );
            for path in [
                &output,
                &output.with_extension("report.json"),
                &output.with_extension("report.md"),
            ] {
                fs::remove_file(path).unwrap();
            }
        }
    });
}

// Section entries must survive a consumer reset, e.g. boss 2 can start from clock zero after 7DE.
#[rstest]
#[case::single_dungeon(GenerateMode::Dungeon, false, false)]
#[case::multi_alliance(GenerateMode::Alliance, true, false)]
#[case::omitted_first_boss(GenerateMode::Dungeon, false, true)]
fn boss_sections_have_separate_clocks_and_wide_entries(
    #[case] mode: GenerateMode,
    #[case] multi: bool,
    #[case] omit_first: bool,
) {
    let mut log = sample();
    log["report"]["endTime"] = json!(9000);
    log["report"]["fights"][0]["endTime"] = json!(9000);
    log["collection"]["endTime"] = json!(9000);
    log["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"gameID":90002,"name":"Later Move","type":"1"}));
    log["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
    log["report"]["fights"][0]["enemyNPCs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"gameID":99903}));
    log["events"] = json!([
        {"timestamp":2000,"type":"cast","sourceID":10,"abilityGameID":90001,"fight":2},
        {"timestamp":4000,"type":"cast","sourceID":10,"abilityGameID":90002,"fight":2},
        {"timestamp":6000,"type":"cast","sourceID":12,"abilityGameID":90001,"fight":2},
        {"timestamp":8000,"type":"cast","sourceID":12,"abilityGameID":90002,"fight":2}
    ]);
    log["collection"]["eventCount"] = json!(4);
    if omit_first {
        for event in log["events"].as_array_mut().unwrap().iter_mut().take(2) {
            event["melee"] = json!(true);
        }
    }
    let mut peer = log.clone();
    peer["report"]["code"] = json!("peer");
    peer["collection"]["reportCode"] = json!("peer");
    let logs = if multi { vec![log, peer] } else { vec![log] };
    with_logs(&logs, |dir| {
        let output = dir.join("sections.yaml");
        generate(dir, &output, mode).unwrap();
        let yaml = fs::read_to_string(&output).unwrap();
        let events = draft_events(&yaml);
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        for (slot, event) in report["slots"].as_array().unwrap().iter().zip(&events) {
            if let Some(at) = slot.get("atMs") {
                assert_eq!(at.as_f64().unwrap(), event["at"].as_f64().unwrap() * 1000.0);
            }
        }
        assert_eq!(
            events
                .iter()
                .map(|event| event["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            if omit_first {
                vec![1000.0, 1002.0]
            } else {
                vec![1000.0, 1002.0, 2000.0, 2002.0]
            }
        );
        assert_eq!(events[0]["sync"]["window"], json!([1000.0, 2.5]));
        let opening = if omit_first { 0 } else { 2 };
        let at = if omit_first { 1000.0 } else { 2000.0 };
        assert_eq!(events[opening]["sync"]["window"], json!([at, 2.5]));
        replay::replay_file(&output, dir, dir.join("sections.replay.json"), None).unwrap();
        // The second entry remains active after zero reset, e.g. replay just its raw boss casts.
        let source = load_one(&dir.join("fight_0.json")).unwrap();
        let signals = replay::signals(&source.log)
            .unwrap()
            .into_iter()
            .filter(|signal| signal.at_ms >= 5000)
            .collect::<Vec<_>>();
        let entry =
            serde_json::from_value::<crate::timeline::Entry>(events[opening].clone()).unwrap();
        let result = crate::timeline::replay::run(
            &serde_saphyr::to_string(&Timeline {
                schema_version: 1,
                reset_on: vec![],
                hide_names: vec![],
                entries: vec![entry],
            })
            .unwrap(),
            &signals[..1],
            source.pull.end_ms,
            &crate::timeline::replay::Evidence {
                expected: BTreeMap::from([(0, BTreeSet::from([signals[0].index]))]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            result.passed,
            "the second entry must be reachable from zero"
        );
    });
}

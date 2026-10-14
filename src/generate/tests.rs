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
#[case::same_tail(false, false, false, false)]
#[case::different_tail(true, false, false, false)]
#[case::different_name(false, true, false, false)]
#[case::id_alternatives_throughout(false, false, true, false)]
#[case::single_remaining_path(false, false, true, true)]
fn branch_id_alternatives_merge_only_identical_paths(
    #[case] different_tail: bool,
    #[case] different_name: bool,
    #[case] tail_id_alternatives: bool,
    #[case] single_path: bool,
) {
    let logs: Vec<_> = (0..if single_path { 4 } else { 5 })
        .map(|variant| {
            let mut log = multi_log(
                &format!("variant-{variant}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (5000 + variant * 100, 11, 0xBEE4 + variant, "cast"),
                    (
                        5600 + variant * 100,
                        11,
                        0xC4E0 + if tail_id_alternatives { variant } else { 0 },
                        "cast",
                    ),
                    (
                        10000 + variant * 100,
                        11,
                        if variant == 4 || different_tail && variant == 3 {
                            90005
                        } else {
                            0xBEED
                        },
                        "cast",
                    ),
                    (12000, 10, 90004, "cast"),
                    (15000, 10, 90008, "cast"),
                ],
                18000,
                true,
            );
            for ability in log["report"]["masterData"]["abilities"]
                .as_array_mut()
                .unwrap()
            {
                if ability["gameID"] == 0xBEE4 + variant {
                    ability["name"] = json!(if different_name && variant == 3 {
                        "Other ability"
                    } else {
                        "Mortifying Flesh"
                    });
                }
                if tail_id_alternatives && ability["gameID"] == 0xC4E0 + variant {
                    ability["name"] = json!("Followup");
                }
            }
            log
        })
        .collect();
    with_logs(&logs, |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        let branches = report["extensions"]["branches"].as_array().unwrap();
        if single_path {
            assert!(branches.is_empty());
            assert!(
                draft_events(&yaml)
                    .iter()
                    .all(|event| event.get("jump").is_none())
            );
            assert!(
                report["slots"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|slot| slot.get("path").is_none())
            );
        } else {
            assert_eq!(branches.len(), 1);
            assert_eq!(
                branches[0]["paths"].as_array().unwrap().len(),
                if different_name { 3 } else { 2 }
            );
        }
        let events = draft_events(&yaml);
        let selector = events
            .iter()
            .find(|event| event["name"] == "Mortifying Flesh")
            .unwrap();
        assert_eq!(
            selector["sync"]["fields"]["id"],
            if different_tail || different_name {
                json!(["BEE4", "BEE5", "BEE6"])
            } else {
                json!(["BEE4", "BEE5", "BEE6", "BEE7"])
            }
        );
        let text = crate::timeline::convert(&yaml).unwrap();
        if tail_id_alternatives {
            let followup = events
                .iter()
                .find(|event| event["name"] == "Followup")
                .unwrap();
            assert_eq!(
                followup["sync"]["fields"]["id"],
                json!(["C4E0", "C4E1", "C4E2", "C4E3"])
            );
            assert!(text.contains("id: [\"C4E0\", \"C4E1\", \"C4E2\", \"C4E3\"]"));
        }
        assert!(text.contains(if different_tail || different_name {
            "id: [\"BEE4\", \"BEE5\", \"BEE6\"]"
        } else {
            "id: [\"BEE4\", \"BEE5\", \"BEE6\", \"BEE7\"]"
        }));
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        for check in report["extensions"]["checks"].as_array().unwrap() {
            assert_eq!(check["passed"], true);
        }
    });
}

#[rstest]
#[case::equivalent(false)]
#[case::different_successor(true)]
fn branch_merges_multiple_id_combinations_per_selector(#[case] different_successor: bool) {
    let logs: Vec<_> = (0..8)
        .map(|variant| {
            let tail = if different_successor && variant == 7 {
                90009
            } else {
                0xBEE8 + (variant / 2 + variant % 2) % 4
            };
            let mut log = multi_log(
                &format!("combination-{variant}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (4500, 10, 0xBEE4 + variant / 2, "begincast"),
                    (5000, 10, 0xBEE4 + variant / 2, "cast"),
                    (5200, 10, tail, "begincast"),
                    (5500, 10, tail, "cast"),
                    (10000, 10, 0xC4E0, "cast"),
                    (10500, 10, 0xBEEC, "cast"),
                    (15000, 10, 0xBEED, "cast"),
                    (18000, 10, 90004, "cast"),
                    (31000, 12, 90005, "cast"),
                    (35000, 12, 90006, "cast"),
                ],
                36000,
                true,
            );
            for ability in log["report"]["masterData"]["abilities"]
                .as_array_mut()
                .unwrap()
            {
                if (0xBEE4..=0xBEEC).contains(&ability["gameID"].as_i64().unwrap()) {
                    ability["name"] = json!("Mortifying Flesh");
                }
            }
            log["report"]["masterData"]["actors"].as_array_mut().unwrap().push(json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
            log["report"]["fights"][0]["enemyNPCs"].as_array_mut().unwrap().push(json!({"id":12,"gameID":99903}));
            log["events"].as_array_mut().unwrap().insert(9, json!({"timestamp":21000,"type":"instancesealupdate","sealType":3,"fight":2}));
            log["collection"]["eventCount"] = json!(12);
            log
        })
        .collect();
    with_logs(&logs, |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Dungeon,
            30.0,
        )
        .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        let branches = report["extensions"]["branches"].as_array().unwrap();
        assert_eq!(branches.is_empty(), !different_successor);
        let text = crate::timeline::convert(&yaml).unwrap();
        if !different_successor {
            assert!(!text.contains("label \"branch-"));
            assert!(text.contains("id: [\"BEE4\", \"BEE5\", \"BEE6\", \"BEE7\"]"));
            assert!(text.contains("id: [\"BEE8\", \"BEE9\", \"BEEA\", \"BEEB\"]"));
        }
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        for check in report["extensions"]["checks"].as_array().unwrap() {
            assert_eq!(check["passed"], true);
        }
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

#[rstest]
#[case::missing_dungeon(GenerateMode::Dungeon, true, false)]
#[case::early_kill_dungeon(GenerateMode::Dungeon, false, false)]
#[case::missing_alliance(GenerateMode::Alliance, true, false)]
#[case::early_kill_alliance(GenerateMode::Alliance, false, false)]
#[case::delayed_death_dungeon(GenerateMode::Dungeon, false, true)]
#[case::delayed_death_alliance(GenerateMode::Alliance, false, true)]
fn replay_boss_suffix_censors_only_after_observed_end(
    #[case] mode: GenerateMode,
    #[case] still_alive: bool,
    #[case] arena_open: bool,
) {
    let training = multi_log(
        "train",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90002, "cast"),
            (9000, 10, 90003, "cast"),
        ],
        11000,
        true,
    );
    let mut holdout = multi_log(
        "holdout",
        &[(1000, 10, 90001, "cast"), (5000, 10, 90002, "cast")],
        11000,
        true,
    );
    if arena_open {
        // Arena opening bounds combat, e.g. FFLogs can deliver the boss's death two seconds later.
        holdout["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":8000,"type":"instancesealupdate","sealType":3,"fight":2}));
    }
    if still_alive || arena_open {
        // Damage proves survival; a delayed death does not, e.g. the missing mechanic is due at 9s.
        holdout["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":11000,"type":if still_alive {"damage"} else {"death"},"targetID":10,"fight":2}));
    }
    holdout["collection"]["eventCount"] = json!(holdout["events"].as_array().unwrap().len());
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        let result = replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None);
        assert_eq!(result.is_ok(), !still_alive, "{result:?}");
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        let summary = &report["pulls"][0]["replay"]["summary"];
        assert_eq!(summary["missing"], usize::from(still_alive));
        assert_eq!(summary["censored"], usize::from(!still_alive));
    });
}

#[rstest]
#[case::dungeon(GenerateMode::Dungeon)]
#[case::alliance(GenerateMode::Alliance)]
fn replay_boss_suffix_follows_the_last_shared_cast_clock(#[case] mode: GenerateMode) {
    let training = multi_log(
        "train",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90002, "cast"),
            (9000, 10, 90003, "cast"),
        ],
        12000,
        true,
    );
    let mut holdout = multi_log(
        "holdout",
        &[(1000, 10, 90001, "cast"), (7000, 10, 90002, "cast")],
        11000,
        true,
    );
    // The last sync delays the suffix to 11s, e.g. boss damage at 10s cannot make it required.
    holdout["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({"timestamp":11000,"type":"damage","targetID":10,"fight":2}));
    holdout["collection"]["eventCount"] = json!(3);
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(report["pulls"][0]["replay"]["summary"]["censored"], 1);
    });
}

#[rstest]
#[case::dungeon(GenerateMode::Dungeon)]
#[case::alliance(GenerateMode::Alliance)]
fn replay_boss_suffix_uses_observed_cast_start_before_termination(#[case] mode: GenerateMode) {
    let training = multi_log(
        "train",
        &[
            (1000, 10, 90001, "cast"),
            (3000, 10, 90002, "begincast"),
            (9000, 10, 90002, "cast"),
        ],
        11000,
        true,
    );
    let mut holdout = multi_log(
        "holdout",
        &[(1000, 10, 90001, "cast"), (7000, 10, 90002, "begincast")],
        11000,
        true,
    );
    // A delayed six-second cast completes at 13s, e.g. damage at 10s precedes that completion.
    holdout["events"]
        .as_array_mut()
        .unwrap()
        .push(json!({"timestamp":11000,"type":"damage","targetID":10,"fight":2}));
    holdout["collection"]["eventCount"] = json!(3);
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(report["pulls"][0]["replay"]["summary"]["censored"], 1);
    });
}

#[rstest]
#[case::dungeon(GenerateMode::Dungeon)]
#[case::alliance(GenerateMode::Alliance)]
fn replay_single_boss_holdout_maps_retries_before_resuming_after_wait(#[case] mode: GenerateMode) {
    let training = multi_log(
        "train",
        &[(1000, 10, 90001, "cast"), (5000, 10, 90002, "cast")],
        6000,
        true,
    );
    let mut holdout = multi_log(
        "holdout",
        &[
            (1000, 10, 90001, "cast"),
            (17000, 10, 90001, "cast"),
            (21000, 10, 90002, "cast"),
        ],
        22000,
        true,
    );
    holdout["events"][0]["sourceInstance"] = json!(1);
    holdout["events"][1]["sourceInstance"] = json!(2);
    holdout["events"][2]["sourceInstance"] = json!(2);
    // Retry indices differ from training, e.g. the successful opening is index 2 after the arena opens.
    holdout["events"].as_array_mut().unwrap().insert(
        1,
        json!({"timestamp":8000,"type":"instancesealupdate","sealType":3,"fight":2}),
    );
    holdout["collection"]["eventCount"] = json!(4);
    with_logs(&[training, holdout], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None).unwrap();
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        let replay = &report["pulls"][0]["replay"];
        assert_eq!(replay["summary"]["matches"], 3);
        assert_eq!(replay["summary"]["wrongMatches"], 0);
        assert_eq!(replay["summary"]["windowMisses"], 0);
        assert_eq!(replay["rows"][0]["expectedEventIndices"], json!([0, 2]));
        assert_eq!(replay["rows"][0]["matches"][1]["clockBeforeMs"], 0);
    });
}

#[rstest]
#[case::single_dungeon("dungeon", false)]
#[case::multi_dungeon("dungeon", true)]
#[case::single_alliance("alliance", false)]
#[case::multi_alliance("alliance", true)]
fn replay_maps_retries_after_overlapping_bosses(#[case] mode: &str, #[case] multi: bool) {
    let mut log = multi_log(
        "overlap",
        &[
            (1000, 10, 90001, "cast"),
            (2000, 12, 90005, "cast"),
            (5000, 10, 90004, "cast"),
            (6000, 12, 90006, "cast"),
            (10000, 13, 90002, "cast"),
            (20000, 13, 90002, "cast"),
            (24000, 13, 90003, "cast"),
        ],
        25000,
        true,
    );
    for id in [12, 13] {
        log["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"name":format!("Boss {id}"),"gameID":99900+id,"type":"NPC","subType":"Boss"}));
        log["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":id,"gameID":99900+id}));
    }
    log["events"][4]["sourceInstance"] = json!(1);
    log["events"][5]["sourceInstance"] = json!(2);
    log["events"][6]["sourceInstance"] = json!(2);
    // Overlapping bosses consume one section, e.g. the third boss's retry belongs to section 1.
    log["events"].as_array_mut().unwrap().insert(
        5,
        json!({"timestamp":16000,"type":"instancesealupdate","sealType":3,"fight":2}),
    );
    log["collection"]["eventCount"] = json!(8);
    let mut logs = vec![log.clone()];
    if multi {
        let mut peer = log.clone();
        peer["report"]["code"] = json!("peer");
        peer["collection"]["reportCode"] = json!("peer");
        logs.push(peer);
    }
    with_logs(&logs, |dir| {
        let yaml = dir.join("overlap.yaml");
        prepare(dir, &yaml, &["--mode", mode]).unwrap();
        with_file(&log, |holdout| {
            let output = dir.join("holdout.replay.json");
            replay::replay_file(&yaml, holdout, &output, None).unwrap();
            let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            let replay = &report["pulls"][0]["replay"];
            assert_eq!(replay["summary"]["matches"], 7);
            assert_eq!(replay["summary"]["wrongMatches"], 0);
            assert!(replay["rows"].as_array().unwrap().iter().any(|row| {
                row["status"] == "matched" && row["expectedEventIndices"] == json!([4, 6])
            }));
        });
    });
}

#[rstest]
#[case::single_dungeon("dungeon", false)]
#[case::multi_dungeon("dungeon", true)]
#[case::single_alliance("alliance", false)]
#[case::multi_alliance("alliance", true)]
fn single_boss_retry_restarts_from_zero(#[case] mode: &str, #[case] multi: bool) {
    let mut log = multi_log(
        "retry",
        &[
            (1000, 10, 90001, "cast"),
            (17000, 10, 90001, "cast"),
            (21000, 10, 90002, "cast"),
        ],
        22000,
        true,
    );
    log["events"][0]["sourceInstance"] = json!(1);
    log["events"][1]["sourceInstance"] = json!(2);
    log["events"][2]["sourceInstance"] = json!(2);
    // Exercise generation from the retry itself, e.g. its selected opening occurs at 17s.
    log["events"].as_array_mut().unwrap().insert(
        1,
        json!({"timestamp":8000,"type":"instancesealupdate","sealType":3,"fight":2}),
    );
    log["collection"]["eventCount"] = json!(4);
    let mut logs = vec![log.clone()];
    if multi {
        let mut peer = log.clone();
        peer["report"]["code"] = json!("peer");
        peer["collection"]["reportCode"] = json!("peer");
        logs.push(peer);
    }
    with_logs(&logs, |dir| {
        let output = dir.join("retry.yaml");
        prepare(dir, &output, &["--mode", mode, "--convert"]).unwrap();
        let events = draft_events(&fs::read_to_string(&output).unwrap());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["sync"]["window"][0], events[0]["at"]);
        assert_eq!(
            events[1]["at"].as_f64().unwrap() - events[0]["at"].as_f64().unwrap(),
            4.0
        );
        with_file(&log, |holdout| {
            let replay_path = dir.join("holdout.replay.json");
            replay::replay_file(&output, holdout, &replay_path, None).unwrap();
            let report: Value = serde_json::from_slice(&fs::read(replay_path).unwrap()).unwrap();
            let result = &report["pulls"][0]["replay"];
            assert_eq!(result["summary"]["matches"], 3);
            assert_eq!(result["summary"]["windowMisses"], 0);
            assert_eq!(result["rows"][0]["matches"][1]["clockBeforeMs"], 0);
        });
    });
}

#[rstest]
#[case::alive_dungeon(GenerateMode::Dungeon, false)]
#[case::defeated_dungeon(GenerateMode::Dungeon, true)]
#[case::alive_alliance(GenerateMode::Alliance, false)]
#[case::defeated_alliance(GenerateMode::Alliance, true)]
fn helper_suffix_checks_bosses_without_casts(#[case] mode: GenerateMode, #[case] defeated: bool) {
    let make = |code, completion| {
        let mut rows = vec![
            (1000, 10, 90001, "cast"),
            (3000, 11, 90003, "begincast"),
            (5000, 10, 90002, "cast"),
        ];
        if completion {
            rows.push((9000, 11, 90003, "cast"));
        }
        let mut log = multi_log(code, &rows, 12000, true);
        log["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .push(
                json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}),
            );
        log["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":12,"gameID":99903}));
        // A melee-only boss still owns combat, e.g. its damage records outlive the caster boss.
        log["events"].as_array_mut().unwrap().extend([
            json!({"timestamp":2000,"type":"damage","targetID":12,"fight":2}),
            json!({"timestamp":if defeated && !completion {7000} else {11000},"type":"damage","targetID":12,"fight":2,"targetResources":{"hitPoints":if defeated && !completion {0} else {1},"maxHitPoints":100}}),
            json!({"timestamp":12000,"type":"instancesealupdate","sealType":3,"fight":2}),
        ]);
        if !completion {
            log["events"].as_array_mut().unwrap().push(json!({"timestamp":9000,"type":"damage","targetID":10,"fight":2,"targetResources":{"hitPoints":0,"maxHitPoints":100}}));
        }
        log["events"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|event| event["timestamp"].as_i64().unwrap());
        log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
        log
    };
    with_logs(&[make("train", true), make("holdout", false)], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        let result = replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None);
        assert_eq!(result.is_ok(), defeated, "{result:?}");
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(
            report["pulls"][0]["replay"]["summary"]["missing"],
            usize::from(!defeated)
        );
        assert_eq!(
            report["pulls"][0]["replay"]["summary"]["censored"],
            usize::from(defeated)
        );
    });
}

#[rstest]
#[case::defeated_dungeon(GenerateMode::Dungeon, true, false)]
#[case::alive_dungeon(GenerateMode::Dungeon, false, false)]
#[case::defeated_alliance(GenerateMode::Alliance, true, false)]
#[case::alive_alliance(GenerateMode::Alliance, false, false)]
#[case::another_boss_alive_dungeon(GenerateMode::Dungeon, true, true)]
#[case::another_boss_alive_alliance(GenerateMode::Alliance, true, true)]
fn replay_helper_suffix_uses_observed_boss_defeat(
    #[case] mode: GenerateMode,
    #[case] defeated: bool,
    #[case] another_boss: bool,
) {
    let make = |code: &str, completion: bool| {
        let mut rows = vec![
            (1000, 10, 90001, "cast"),
            (3000, 11, 90003, "begincast"),
            (5000, 10, 90002, "cast"),
        ];
        if completion {
            rows.push((9000, 11, 90003, "cast"));
        }
        if another_boss {
            rows.extend([(2000, 12, 90004, "cast"), (8000, 12, 90004, "cast")]);
        }
        rows.sort_by_key(|row| row.0);
        let mut log = multi_log(code, &rows, 11000, true);
        if another_boss {
            log["report"]["masterData"]["actors"].as_array_mut().unwrap().push(
                json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}),
            );
            log["report"]["fights"][0]["enemyNPCs"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":12,"gameID":99903}));
        }
        if !completion {
            // Defeat precedes arena opening, e.g. a cancelled helper is due between those two observations.
            log["events"].as_array_mut().unwrap().push(json!({"timestamp":9990,"type":"damage","targetID":10,"fight":2,"targetResources":{"hitPoints":if defeated {0} else {1},"maxHitPoints":100}}));
        }
        log["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":10001,"type":"instancesealupdate","sealType":3,"fight":2}));
        log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
        log
    };
    with_logs(&[make("train", true), make("holdout", false)], |dir| {
        let yaml = dir.join("draft.yaml");
        generate(dir.join("fight_0.json"), &yaml, mode).unwrap();
        let output = dir.join("replay.json");
        let result = replay::replay_file(&yaml, dir.join("fight_1.json"), &output, None);
        let censored = defeated && !another_boss;
        assert_eq!(result.is_ok(), censored, "{result:?}");
        let report: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(
            report["pulls"][0]["replay"]["summary"]["censored"],
            usize::from(censored)
        );
        assert_eq!(
            report["pulls"][0]["replay"]["summary"]["missing"],
            usize::from(!censored)
        );
    });
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

    use usage_rs::Run;

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
fn prepare_rejects_a_draft_without_mechanics() {
    let logs = [90001, 90002].map(|ability| {
        multi_log(
            &format!("report{ability}"),
            &[(1000, 10, ability, "cast")],
            5000,
            true,
        )
    });
    with_logs(&logs, |dir| {
        let output = dir.join("empty.yaml");
        let error = prepare(dir, &output, &["--convert"]).unwrap_err();
        assert!(
            error.to_string().contains("No generated event rows"),
            "{error:#}"
        );
        assert!(output.exists());
        assert!(output.with_extension("report.json").exists());
        assert!(!output.with_extension("txt").exists());
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
fn multi_sync_uses_each_pulls_observed_time_for_collisions() {
    // An 8s drift must not make the median time collide with a different occurrence in the early pull.
    let make = |code, delay: i64| {
        multi_log(
            code,
            &[
                (1000, 10, 90001, "cast"),
                (10000 + delay, 10, 90002, "cast"),
                (18100 + delay, 10, 90002, "cast"),
                (30000 + delay, 10, 90004, "cast"),
            ],
            40000,
            true,
        )
    };
    with_logs(&[make("a", 0), make("b", 8100), make("c", 8100)], |path| {
        let (yaml, report) = multi::build(
            input::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
            30.0,
        )
        .unwrap();
        assert_eq!(report["syncConflicts"], json!([]));
        assert_eq!(report["extensions"]["accepted"], true);
        assert!(
            draft_events(&yaml)
                .iter()
                .all(|event| event["sync"]["enabled"] != false)
        );
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
fn omitted_single_instance_matches_explicit_instance_in_generation_and_holdout() {
    let rows = [(1000, 10, 90001, "cast"), (4000, 10, 90002, "cast")];
    let single = multi_log("single", &rows, 5000, true);
    let mut instanced = multi_log("instanced", &rows, 5000, true);
    // FFLogs omits sourceInstance for a sole actor; a retry reports its new instance, e.g. 2.
    for event in instanced["events"].as_array_mut().unwrap() {
        event["sourceInstance"] = json!(2);
    }
    with_logs(&[single.clone(), instanced.clone()], |dir| {
        let output = dir.join("instances.yaml");
        prepare(dir, &output, &["--mode", "dungeon", "--convert"]).unwrap();
        assert_eq!(draft_events(&fs::read_to_string(&output).unwrap()).len(), 2);
    });
    with_file(&single, |input| {
        let output = input.with_extension("yaml");
        generate(input, &output, GenerateMode::Dungeon).unwrap();
        with_file(&instanced, |holdout| {
            replay::replay_file(&output, holdout, input.with_extension("replay.json"), None)
                .unwrap();
        });
    });
}

#[test]
fn virtual_helpers_match_across_reports_and_staggered_instances() {
    let rows = [
        (1000, 10, 90001, "cast"),
        (3000, 11, 90002, "cast"),
        (3000, 11, 90002, "cast"),
        (7000, 10, 90004, "cast"),
    ];
    let mut a = multi_log("a", &rows, 8000, true);
    a["report"]["masterData"]["actors"][1]["gameID"] = json!(2000011);
    a["report"]["fights"][0]["enemyNPCs"][1]["gameID"] = json!(2000011);
    a["events"][1]["sourceInstance"] = json!(1);
    a["events"][2]["sourceInstance"] = json!(2);
    let mut b = a.clone();
    b["report"]["code"] = json!("b");
    b["collection"]["reportCode"] = json!("b");
    b["report"]["masterData"]["actors"][1]["id"] = json!(21);
    b["report"]["masterData"]["actors"][1]["gameID"] = json!(2000021);
    b["report"]["fights"][0]["enemyNPCs"][1] = json!({"id":21,"gameID":2000021});
    b["events"][1]["sourceID"] = json!(21);
    b["events"][2]["sourceID"] = json!(21);
    b["events"][2]["timestamp"] = json!(4045);
    with_logs(&[a, b], |dir| {
        let (yaml, report) = multi::build(
            input::select_group(dir, None, None, None).unwrap(),
            GenerateMode::Dungeon,
            30.0,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml).len(), 3);
        assert_eq!(
            report["slots"][1]["samples"][1]["eventIndices"],
            json!([1, 2])
        );
        assert_eq!(
            report["slots"][1]["samples"][1]["instanceIds"],
            json!([1, 2])
        );
    });
}

#[rstest]
#[case::single(GenerateMode::Dungeon, false)]
#[case::multi(GenerateMode::Dungeon, true)]
#[case::alliance(GenerateMode::Alliance, true)]
fn boss_retries_and_travel_preserve_local_timing(#[case] mode: GenerateMode, #[case] multi: bool) {
    let make = |code, retry| {
        let rows = if retry {
            vec![
                (1000, 10, 90001, "cast"),
                (10000, 10, 90001, "cast"),
                (14000, 10, 90002, "cast"),
                (70000, 12, 90003, "cast"),
                (74000, 12, 90004, "cast"),
            ]
        } else {
            vec![
                (1000, 10, 90001, "cast"),
                (5000, 10, 90002, "cast"),
                (20000, 12, 90003, "cast"),
                (24000, 12, 90004, "cast"),
            ]
        };
        let mut log = multi_log(code, &rows, 80000, true);
        log["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .push(
                json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}),
            );
        log["report"]["fights"][0]["enemyNPCs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":12,"gameID":99903}));
        if retry {
            log["events"][0]["sourceInstance"] = json!(1);
            log["events"][1]["sourceInstance"] = json!(2);
            log["events"][2]["sourceInstance"] = json!(2);
            log["events"]
                .as_array_mut()
                .unwrap()
                .push(json!({"timestamp":7000,"type":"instancesealupdate","fight":2,"sealType":3}));
        }
        log["events"].as_array_mut().unwrap().push(json!({"timestamp": if retry {16000} else {7000},"type":"instancesealupdate","fight":2,"sealType":3}));
        log["events"]
            .as_array_mut()
            .unwrap()
            .sort_by_key(|event| event["timestamp"].as_i64().unwrap());
        log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
        log
    };
    let mut logs = vec![make("retry", true)];
    if multi {
        logs.push(make("clear", false));
    }
    with_logs(&logs, |dir| {
        let output = dir.join("bosses.yaml");
        let mode = if mode == GenerateMode::Dungeon {
            "dungeon"
        } else {
            "alliance"
        };
        prepare(dir, &output, &["--mode", mode, "--convert"]).unwrap();
        let events = draft_events(&fs::read_to_string(&output).unwrap());
        assert_eq!(events.len(), 4);
        for pair in events.as_chunks::<2>().0 {
            assert_eq!(
                pair[1]["at"].as_f64().unwrap() - pair[0]["at"].as_f64().unwrap(),
                4.0
            );
            assert_eq!(pair[0]["sync"]["window"][0], pair[0]["at"]);
        }
        let replay: Value =
            serde_json::from_slice(&fs::read(output.with_extension("replay.json")).unwrap())
                .unwrap();
        let retry = replay["pulls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pull| pull["report"] == "retry")
            .unwrap();
        assert_eq!(retry["replay"]["summary"]["matches"], 5);
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        let input = if multi {
            report["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|input| input["input"]["report"] == "retry")
                .unwrap()
        } else {
            &report
        };
        assert_eq!(input["bossAttempts"].as_array().unwrap().len(), 2);
        assert_eq!(input["bossSegments"][0]["startMs"], 10000);
    });
}

#[test]
fn boss_repeat_ties_use_unique_observed_timing_after_swapped_casts() {
    let a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90002, "cast"),
            (10000, 10, 90003, "cast"),
            (15000, 10, 90001, "cast"),
            (20000, 10, 90004, "cast"),
        ],
        21000,
        true,
    );
    let b = multi_log(
        "b",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90003, "cast"),
            (10000, 10, 90002, "cast"),
            (15000, 10, 90001, "cast"),
            (20000, 10, 90004, "cast"),
            (30000, 10, 90001, "cast"),
            (34000, 10, 90003, "cast"),
            (39000, 10, 90002, "cast"),
            (44000, 10, 90001, "cast"),
            (49000, 10, 90004, "cast"),
        ],
        50000,
        true,
    );
    with_logs(&[a, b], |dir| {
        let mut pulls = Vec::new();
        for file in ["fight_0.json", "fight_1.json"] {
            let source = load_one(&dir.join(file)).unwrap();
            let report = draft::build_single(&source, GenerateMode::Dungeon)
                .unwrap()
                .report;
            let mut pull = source.pull;
            input::filter_boss_spans(&mut pull, &report).unwrap();
            pulls.push(pull);
        }
        let (relation, _) = multi::correspondence(&pulls[0], &pulls[1]).unwrap();
        assert_eq!(relation.matched.get(&3), Some(&3));
        assert_eq!(relation.matched.get(&4), Some(&4));
    });
}

#[rstest]
#[case::flat_dungeon("dungeon", false)]
#[case::nested_dungeon("dungeon", true)]
#[case::flat_alliance("alliance", false)]
#[case::nested_alliance("alliance", true)]
fn later_boss_opening_paths_are_preserved(#[case] mode: &str, #[case] nested: bool) {
    let choices = if nested {
        vec![(90002, 90003), (90003, 90002), (90002, 90007)]
    } else {
        vec![(90002, 90003), (90003, 90002)]
    };
    let logs: Vec<_> = choices.into_iter().enumerate().map(|(i, (x, y))| {
        let mut log = multi_log(&format!("path-{i}"), &[
            (1000, 10, 90001, "cast"), (5000, 10, 90004, "cast"),
            (6000, 11, 90009, "cast"), (31000, 12, x, "cast"),
            (35000, 12, y, "cast"), (39000, 12, 90006, "cast"),
            (43000, 12, 90008, "cast"),
        ], 44000, true);
        log["report"]["masterData"]["actors"].as_array_mut().unwrap().push(json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
        log["report"]["fights"][0]["enemyNPCs"].as_array_mut().unwrap().push(json!({"id":12,"gameID":99903}));
        // Keep the first boss's trailing helper while recovering boss 2's choices before its first common cast.
        log["events"].as_array_mut().unwrap().insert(3, json!({"timestamp":8000,"type":"instancesealupdate","sealType":3,"fight":2}));
        log["collection"]["eventCount"] = json!(8);
        log
    }).collect();
    with_logs(&logs, |dir| {
        let output = dir.join("branches.yaml");
        prepare(dir, &output, &["--mode", mode, "--convert"]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        assert_eq!(
            report["extensions"]["branches"].as_array().unwrap().len(),
            if nested { 2 } else { 1 }
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]), "{coverage}");
        }
        for (i, log) in logs.iter().enumerate() {
            with_file(log, |holdout| {
                let replay_path = dir.join(format!("holdout-{i}.replay.json"));
                replay::replay_file(&output, holdout, &replay_path, None).unwrap();
                let replay: Value =
                    serde_json::from_slice(&fs::read(replay_path).unwrap()).unwrap();
                assert_eq!(replay["pulls"][0]["replay"]["summary"]["matches"], 7);
            });
        }
    });
}

#[test]
fn multi_boss_recovers_independent_paths_in_each_boss() {
    let mut logs: Vec<_> = [(90002, 90003), (90003, 90002), (90002, 90007)]
        .into_iter()
        .enumerate()
        .map(|(i, (x, y))| {
            let mut log = multi_log(
                &format!("path-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (5000, 10, x, "cast"),
                    (9000, 10, y, "cast"),
                    (13000, 10, 90004, "cast"),
                    (31000, 12, 90005, "cast"),
                    (35000, 12, x, "cast"),
                    (39000, 12, y, "cast"),
                    (43000, 12, 90006, "cast"),
                ],
                44000,
                true,
            );
            log["report"]["masterData"]["actors"].as_array_mut().unwrap().push(
                json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}),
            );
            log["report"]["fights"][0]["enemyNPCs"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":12,"gameID":99903}));
            log
        })
        .collect();
    // A retry chooses another observed outer path before the clear takes a nested choice.
    let earlier = logs[1]["events"].as_array().unwrap()[..4].to_vec();
    for event in logs[0]["events"].as_array_mut().unwrap() {
        event["timestamp"] = json!(event["timestamp"].as_i64().unwrap() + 20000);
        if event["sourceID"] == 10 {
            event["sourceInstance"] = json!(2);
        }
    }
    let mut events = earlier;
    events.push(json!({"timestamp":15000,"type":"instancesealupdate","sealType":3,"fight":2}));
    events.extend(logs[0]["events"].as_array().unwrap().iter().cloned());
    logs[0]["events"] = json!(events);
    logs[0]["collection"]["eventCount"] = json!(events.len());
    for field in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *logs[0].pointer_mut(field).unwrap() = json!(65000);
    }
    with_logs(&logs, |dir| {
        let output = dir.join("branches.yaml");
        prepare(dir, &output, &["--mode", "dungeon"]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        assert!(
            report["extensions"]["branches"].as_array().unwrap().len() >= 2,
            "{}",
            report["extensions"]
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[test]
fn multi_boss_keeps_packet_waves_optional_helpers_and_short_clear_suffixes() {
    let make =
        |long: bool| {
            let mut rows = vec![(1000, 10, 90001, "cast")];
            for at in [3000, 5000] {
                for _ in 0..if long { 2 } else { 3 } {
                    rows.push((at - 1000, 11, 90002, "begincast"));
                    rows.push((at, 11, 90002, "cast"));
                }
            }
            if long {
                rows.push((7000, 11, 90008, "cast"));
            }
            rows.push((9000, 10, 90004, "cast"));
            if long {
                rows.extend([
                    (11000, 11, 90002, "cast"),
                    (13000, 11, 90002, "cast"),
                    (15000, 10, 90005, "cast"),
                    (16000, 11, 90002, "cast"),
                ]);
            }
            rows.extend([(20000, 12, 90006, "cast"), (24000, 12, 90007, "cast")]);
            rows.sort_by_key(|row| row.0);
            let mut log = multi_log(if long { "long" } else { "short" }, &rows, 25000, true);
            log["report"]["masterData"]["actors"][1]["gameID"] = json!(2000011);
            log["report"]["fights"][0]["enemyNPCs"][1]["gameID"] = json!(2000011);
            log["report"]["masterData"]["actors"].as_array_mut().unwrap().push(
            json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}),
        );
            log["report"]["fights"][0]["enemyNPCs"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":12,"gameID":99903}));
            let mut instance = 0;
            for event in log["events"].as_array_mut().unwrap() {
                if event["sourceID"] == 11 && event["type"] == "cast" {
                    instance += 1;
                    event["sourceInstance"] = json!(instance);
                }
            }
            if long {
                // The effect at 16s survives the boss's last interaction at 15s until the arena opens.
                log["events"].as_array_mut().unwrap().push(
                    json!({"timestamp":20000,"type":"instancesealupdate","sealType":3,"fight":2}),
                );
                log["events"]
                    .as_array_mut()
                    .unwrap()
                    .sort_by_key(|event| event["timestamp"].as_i64().unwrap());
                log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
            }
            log
        };
    with_logs(&[make(true), make(false)], |dir| {
        let output = dir.join("waves.yaml");
        prepare(dir, &output, &["--mode", "dungeon"]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(
                coverage["omittedEventIndices"],
                json!([]),
                "{coverage}, {}",
                report["extensions"]
            );
        }
        let events = draft_events(&fs::read_to_string(output).unwrap());
        let optional = events
            .iter()
            .find(|entry| entry["sync"]["fields"]["id"] == "15F98")
            .unwrap();
        assert_eq!(optional["sync"]["enabled"], false);
        assert!(
            report["slots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|slot| slot["evidence"] == "conditionalInstantHelper")
        );
        let waves: Vec<_> = events
            .iter()
            .filter(|entry| entry["sync"]["fields"]["id"] == "15F92")
            .collect();
        assert_eq!(waves.len(), 5);
        assert!(waves.iter().all(|entry| entry["sync"]["enabled"] != false));
    });
}

#[test]
fn multi_boss_keeps_repeat_compression_and_raw_replay() {
    let mut log = repeat_log("a", 2, true);
    log["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"name":"Second Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
    log["report"]["fights"][0]["enemyNPCs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"gameID":99903}));
    log["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"gameID":91005,"name":"Final Move","type":"1"}));
    log["events"].as_array_mut().unwrap().push(
        json!({"timestamp":51000,"type":"cast","sourceID":12,"abilityGameID":91005,"fight":2}),
    );
    log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
    for field in [
        "/report/endTime",
        "/report/fights/0/endTime",
        "/collection/endTime",
    ] {
        *log.pointer_mut(field).unwrap() = json!(52000);
    }
    let mut peer = log.clone();
    peer["report"]["code"] = json!("b");
    peer["collection"]["reportCode"] = json!("b");
    peer["report"]["fights"][0]["kill"] = json!(false);
    with_logs(&[log, peer], |dir| {
        let output = dir.join("repeat.yaml");
        prepare(dir, &output, &["--mode", "alliance"]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["repeats"]["accepted"], true, "{}", report["repeats"]);
    });
}

#[rstest]
#[case::simultaneous(0)]
#[case::staggered(1)]
fn simultaneous_instances_are_preserved_without_inflating_pull_sample_counts(#[case] stagger: i64) {
    let mut a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (3000, 11, 90002, "cast"),
            (3000 + stagger, 11, 90002, "cast"),
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
        // A collapsed wave is represented in full, e.g. both helper instances belong to one row.
        let output = path.join("wave.yaml");
        prepare(path, &output, &[]).unwrap();
        let replay: Value =
            serde_json::from_slice(&fs::read(output.with_extension("replay.json")).unwrap())
                .unwrap();
        for pull in replay["pulls"].as_array().unwrap() {
            assert_eq!(pull["unrepresentedEventIndices"], json!([]));
        }
    });
}

#[test]
fn raid_phase_clocks_prevent_matching_a_choice_to_a_later_round() {
    let mut a = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90004, "cast"),
            (24000, 10, 90002, "cast"),
            (34000, 10, 90003, "cast"),
            (40000, 10, 90005, "cast"),
        ],
        41000,
        true,
    );
    let mut b = multi_log(
        "b",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90004, "cast"),
            (34000, 10, 90003, "cast"),
            (44000, 10, 90002, "cast"),
            (50000, 10, 90005, "cast"),
        ],
        51000,
        true,
    );
    a["report"]["fights"][0]["phaseTransitions"] =
        json!([{"id":7,"startTime":1000},{"id":3,"startTime":22000}]);
    b["report"]["fights"][0]["phaseTransitions"] =
        json!([{"id":7,"startTime":1000},{"id":3,"startTime":32000}]);
    with_logs(&[a, b], |dir| {
        let a = load_one(&dir.join("fight_0.json")).unwrap();
        let b = load_one(&dir.join("fight_1.json")).unwrap();
        let (relation, _) = multi::correspondence(&a.pull, &b.pull).unwrap();
        assert_eq!(relation.matched.get(&4), Some(&4));
        assert!(!relation.matched.contains_key(&2));
        assert!(!relation.matched.contains_key(&3));
        let output = dir.join("phases.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[test]
fn raid_phase_suffix_needs_explicit_defeat_of_every_overlapping_boss() {
    let mut long = multi_log(
        "long",
        &[
            (1000, 10, 90001, "cast"),
            (15000, 10, 90002, "cast"),
            (30000, 10, 90003, "cast"),
        ],
        31000,
        true,
    );
    long["report"]["fights"][0]["phaseTransitions"] =
        json!([{"id":1,"startTime":1000},{"id":2,"startTime":11000}]);
    let mut short = multi_log(
        "short",
        &[(1000, 10, 90001, "cast"), (15000, 10, 90002, "cast")],
        21000,
        true,
    );
    short["report"]["fights"][0]["phaseTransitions"] =
        long["report"]["fights"][0]["phaseTransitions"].clone();
    for (defeat, overlapping, censored) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let mut peer = short.clone();
        if defeat {
            peer["events"].as_array_mut().unwrap().push(json!({"timestamp":20000,"type":"damage","fight":2,"sourceID":11,"targetID":10,"targetResources":{"hitPoints":0,"maxHitPoints":100}}));
        }
        if overlapping {
            peer["report"]["masterData"]["actors"].as_array_mut().unwrap().push(json!({"id":12,"name":"Other Boss","gameID":99903,"type":"NPC","subType":"Boss"}));
            peer["report"]["fights"][0]["enemyNPCs"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id":12,"gameID":99903}));
            peer["events"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::to_value(cast_event(21000, 12, 90002, "cast")).unwrap());
        }
        peer["collection"]["eventCount"] = json!(peer["events"].as_array().unwrap().len());
        with_logs(&[long.clone(), peer], |dir| {
            let a = load_one(&dir.join("fight_0.json")).unwrap();
            let b = load_one(&dir.join("fight_1.json")).unwrap();
            let (relation, _) = multi::correspondence(&a.pull, &b.pull).unwrap();
            assert_eq!(relation.censored.contains(&2), censored);
        });
    }
}

#[test]
fn raid_packet_fanout_does_not_create_a_branch_or_untracked_casts() {
    let logs: Vec<_> = [2, 3]
        .into_iter()
        .enumerate()
        .map(|(i, count)| {
            let mut rows = vec![(1000, 10, 90001, "cast")];
            rows.extend((0..count).map(|packet| (3000 + packet * 180, 11, 90002, "cast")));
            rows.push((8000, 10, 90004, "cast"));
            let mut log = multi_log(&format!("wave-{i}"), &rows, 9000, true);
            for packet in 0..count {
                log["events"][1 + usize::try_from(packet).unwrap()]["sourceInstance"] =
                    json!(packet + 1);
            }
            log
        })
        .collect();
    with_logs(&logs, |dir| {
        let output = dir.join("wave.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["slots"].as_array().unwrap().len(), 3);
        assert!(
            report["extensions"]["branches"]
                .as_array()
                .is_none_or(Vec::is_empty)
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        let replay: Value =
            serde_json::from_slice(&fs::read(output.with_extension("replay.json")).unwrap())
                .unwrap();
        for pull in replay["pulls"].as_array().unwrap() {
            assert_eq!(pull["unrepresentedEventIndices"], json!([]));
        }
    });
}

#[rstest]
#[case::completion_alias(false)]
#[case::start_alias(true)]
fn raid_discriminator_uses_cast_start_when_helper_shares_its_completion_signature(
    #[case] start_alias: bool,
) {
    let mut logs: Vec<_> = [(90002, 90005), (90003, 90006)]
        .into_iter()
        .enumerate()
        .map(|(i, (choice, tail))| {
            multi_log(
                &format!("path-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (3000, 10, choice, "begincast"),
                    (6000, 10, choice, "cast"),
                    (8000, 10, tail, "cast"),
                    (10000, 10, 90004, "cast"),
                ],
                11000,
                true,
            )
        })
        .collect();
    let log = &mut logs[0];
    log["report"]["masterData"]["actors"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"name":"Boss","gameID":99903,"type":"NPC","subType":"NPC"}));
    log["report"]["fights"][0]["enemyNPCs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":12,"gameID":99903}));
    for instance in 1..=2 {
        let mut packet =
            serde_json::to_value(cast_event(7400 + instance, 12, 90002, "cast")).unwrap();
        packet["sourceInstance"] = json!(instance);
        log["events"].as_array_mut().unwrap().push(packet);
    }
    if start_alias {
        let mut start = serde_json::to_value(cast_event(3999, 12, 90002, "begincast")).unwrap();
        start["sourceInstance"] = json!(1);
        log["events"].as_array_mut().unwrap().push(start);
    } else {
        // A different, earlier helper start keeps these completions separate from the boss's cast.
        for instance in 1..=2 {
            let mut start = serde_json::to_value(cast_event(1200, 12, 90002, "begincast")).unwrap();
            start["sourceInstance"] = json!(instance);
            log["events"].as_array_mut().unwrap().push(start);
        }
    }
    log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
    with_logs(&logs, |dir| {
        let output = dir.join("start.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        assert_eq!(
            report["extensions"]["branches"].as_array().unwrap().len(),
            1
        );
        let events = draft_events(&fs::read_to_string(&output).unwrap());
        assert_eq!(
            events
                .iter()
                .filter(|event| event["sync"]["log"] == "StartsUsing")
                .count(),
            if start_alias { 0 } else { 2 }
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        let mut holdout = logs[0].clone();
        holdout["report"]["code"] = json!("holdout");
        holdout["collection"]["reportCode"] = json!("holdout");
        with_file(&holdout, |input| {
            replay::replay_file(&output, input, dir.join("holdout.replay.json"), None).unwrap();
        });
        // Enemy aliases are evidence; a friendly actor with the same name must still expose accidental activation.
        holdout["report"]["masterData"]["actors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":13,"name":"Boss","gameID":99904,"type":"Player","subType":""}));
        holdout["events"].as_array_mut().unwrap().push(
            serde_json::to_value(if start_alias {
                cast_event(6999, 13, 90002, "cast")
            } else {
                cast_event(3998, 13, 90002, "begincast")
            })
            .unwrap(),
        );
        holdout["collection"]["eventCount"] = json!(holdout["events"].as_array().unwrap().len());
        with_file(&holdout, |input| {
            assert!(
                replay::replay_file(&output, input, dir.join("friendly.replay.json"), None)
                    .is_err()
            );
        });
    });
}

#[test]
fn raid_joint_packet_choices_preserve_both_mechanics_and_accept_reversed_holdout_order() {
    let logs: Vec<_> = [
        (90002, 90012),
        (90002, 90013),
        (90003, 90012),
        (90003, 90013),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (a, b))| {
        let mut rows = vec![
            (1000, 10, 90001, "cast"),
            (3000, 10, a, "begincast"),
            (3000, 11, b, "begincast"),
            (6000, 10, a, "cast"),
            (6000, 11, b, "cast"),
            (10000, 10, 90004, "cast"),
        ];
        if i % 2 == 1 {
            rows.swap(3, 4);
        }
        multi_log(&format!("joint-{i}"), &rows, 11000, true)
    })
    .collect();
    with_logs(&logs, |dir| {
        let output = dir.join("joint.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        assert!(
            report["extensions"]["branches"]
                .as_array()
                .unwrap()
                .iter()
                .any(|branch| branch["packetWave"] == true)
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        // A different packet order must still select a path with both choices, e.g. helper B arrives before boss A.
        let mut holdout = logs[0].clone();
        holdout["report"]["code"] = json!("joint-holdout");
        holdout["collection"]["reportCode"] = json!("joint-holdout");
        holdout["events"].as_array_mut().unwrap().swap(3, 4);
        with_file(&holdout, |input| {
            replay::replay_file(&output, input, dir.join("holdout.replay.json"), None).unwrap();
        });
    });
}

#[rstest]
#[case::paired(false, 1, 500, 3)]
#[case::orphan(true, 1, 500, 3)]
#[case::delayed_copy(false, 600, 600, 3)]
#[case::later_followup(true, 1, 2000, 4)]
#[case::separate_cast(false, 2000, 2000, 4)]
#[case::distinct_starts_same_tick(false, 2000, 0, 4)]
#[case::distinct_starts_same_wave(false, -2000, 100, 4)]
fn raid_late_completion_aliases_share_the_observed_start_and_keep_raw_coverage(
    #[case] orphan: bool,
    #[case] start_offset: i64,
    #[case] completion_offset: i64,
    #[case] rows: usize,
) {
    let logs: Vec<_> = [0, 1]
        .into_iter()
        .map(|i| {
            let mut log = multi_log(
                &format!("alias-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (3000, 10, 90002, "begincast"),
                    (3000 + start_offset, 11, 90012, "begincast"),
                    (6000, 10, 90002, "cast"),
                    (6000 + completion_offset, 11, 90012, "cast"),
                    (10000, 10, 90004, "cast"),
                ],
                11000,
                true,
            );
            log["report"]["masterData"]["actors"][1]["name"] = json!("Boss");
            if orphan {
                log["events"].as_array_mut().unwrap().remove(2);
                log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
            }
            for ability in log["report"]["masterData"]["abilities"]
                .as_array_mut()
                .unwrap()
            {
                if ability["gameID"] == 90012 {
                    ability["name"] = json!("Ability 90002");
                }
            }
            log
        })
        .collect();
    with_logs(&logs, |dir| {
        let output = dir.join("aliases.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["slots"].as_array().unwrap().len(), rows);
        assert_eq!(
            report["slots"][1]["abilityIds"],
            if rows == 3 {
                json!([90002, 90012])
            } else {
                json!([90002])
            }
        );
        assert_eq!(report["slots"][1]["time"]["sampleCount"], 2);
        assert_ne!(
            draft_events(&fs::read_to_string(&output).unwrap())[1]["sync"]["enabled"],
            false
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        if rows == 4 {
            // Independent casts are mandatory, e.g. removing the helper must not satisfy an ID alternative.
            let mut holdout = logs[0].clone();
            holdout["report"]["code"] = json!("missing-cast");
            holdout["collection"]["reportCode"] = json!("missing-cast");
            holdout["events"]
                .as_array_mut()
                .unwrap()
                .retain(|event| event["sourceID"] != 11);
            holdout["collection"]["eventCount"] =
                json!(holdout["events"].as_array().unwrap().len());
            with_file(&holdout, |input| {
                let replay_output = dir.join("missing.replay.json");
                assert!(replay::replay_file(&output, input, &replay_output, None).is_err());
                let result: Value =
                    serde_json::from_slice(&fs::read(replay_output).unwrap()).unwrap();
                assert_eq!(result["pulls"][0]["replay"]["summary"]["missing"], 1);
            });
        }
    });
}

#[test]
fn raid_truncated_branch_reactions_use_the_observed_discriminator_clock() {
    let first = multi_log(
        "x",
        &[
            (1000, 10, 90001, "cast"),
            (2000, 10, 90002, "cast"),
            (5000, 10, 90005, "cast"),
            (6500, 10, 90007, "cast"),
            (8000, 10, 90004, "cast"),
        ],
        9000,
        true,
    );
    let second = multi_log(
        "y",
        &[
            (1000, 10, 90001, "cast"),
            (2000, 10, 90003, "cast"),
            (5000, 10, 90006, "cast"),
            (6500, 10, 90007, "cast"),
            (8000, 10, 90004, "cast"),
        ],
        9000,
        true,
    );
    let mut wipe = multi_log(
        "wipe",
        &[
            (1000, 10, 90001, "cast"),
            (2000, 10, 90002, "cast"),
            (3000, 11, 90012, "cast"),
            (3001, 11, 90012, "cast"),
        ],
        3500,
        false,
    );
    wipe["events"][2]["sourceInstance"] = json!(1);
    wipe["events"][3]["sourceInstance"] = json!(2);
    with_logs(&[first, second, wipe], |dir| {
        let output = dir.join("partial.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(
            report["extensions"]["accepted"], true,
            "{}",
            report["extensions"]
        );
        assert_eq!(
            report["extensions"]["branches"].as_array().unwrap().len(),
            1,
            "{}",
            report["extensions"]
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[rstest]
#[case::one_instance(false)]
#[case::different_instances(true)]
fn raid_repeated_started_casts_from_one_actor_are_not_merged_as_copies(
    #[case] different_instances: bool,
) {
    let rows = [
        (500, 10, 90002, "begincast"),
        (1000, 10, 90002, "cast"),
        (2500, 10, 90002, "begincast"),
        (3000, 10, 90002, "cast"),
        (8000, 10, 90004, "cast"),
    ];
    let mut logs = [
        multi_log("a", &rows, 9000, true),
        multi_log("b", &rows, 9000, true),
    ];
    if different_instances {
        for log in &mut logs {
            for (index, instance) in [(0, 1), (1, 1), (2, 2), (3, 2)] {
                log["events"][index]["sourceInstance"] = json!(instance);
            }
        }
    }
    with_logs(&logs, |dir| {
        let output = dir.join("repeated.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["slots"].as_array().unwrap().len(), 3);
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[test]
fn single_draft_replay_keeps_distinct_alias_rows() {
    let mut log = sample();
    log["report"]["masterData"]["actors"][1]["name"] = json!("Boss");
    log["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"gameID":90002,"name":"Move","type":"1"}));
    log["events"][0]["abilityGameID"] = json!(90002);
    with_file(&log, |input| {
        let output = input.with_extension("yaml");
        prepare(input, &output, &[]).unwrap();
        let replay: Value =
            serde_json::from_slice(&fs::read(output.with_extension("replay.json")).unwrap())
                .unwrap();
        assert_eq!(replay["pulls"][0]["replay"]["passed"], true);
        assert_eq!(replay["pulls"][0]["unrepresentedEventIndices"], json!([]));
    });
}

#[test]
fn raid_observed_suffix_is_preserved_without_a_common_successor() {
    let mut short = multi_log(
        "short",
        &[(1000, 10, 90001, "cast"), (5000, 10, 90002, "cast")],
        6000,
        true,
    );
    let mut long = multi_log(
        "long",
        &[
            (1000, 10, 90001, "cast"),
            (5000, 10, 90002, "cast"),
            (6000, 10, 90008, "begincast"),
            (8000, 10, 90008, "cast"),
        ],
        9000,
        false,
    );
    for log in [&mut short, &mut long] {
        log["report"]["fights"][0]["phaseTransitions"] = json!([{"id":1,"startTime":1000}]);
    }
    with_logs(&[short, long], |dir| {
        let output = dir.join("suffix.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        let suffix = report["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["abilityIds"] == json!([90008]))
            .unwrap();
        assert_eq!(suffix["evidence"], "observedSuffix");
        assert_eq!(
            draft_events(&fs::read_to_string(&output).unwrap())
                .iter()
                .find(|event| event["name"] == "Ability 90008")
                .unwrap()["sync"]["enabled"],
            false
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[test]
fn raid_unresolved_phase_preserves_verified_branches_and_every_observed_cast() {
    let mut logs: Vec<_> = [(90002, 90005, 90012, 90015), (90003, 90006, 90013, 90016)]
        .into_iter()
        .enumerate()
        .map(|(i, (a, b, c, d))| {
            multi_log(
                &format!("full-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (3000, 10, a, "cast"),
                    (5000, 10, b, "cast"),
                    (10000, 10, 90004, "cast"),
                    (20000, 10, 90011, "cast"),
                    (22000, 10, c, "cast"),
                    (25000, 10, d, "cast"),
                    (30000, 10, 90014, "cast"),
                ],
                31000,
                true,
            )
        })
        .collect();
    logs.push(multi_log(
        "wipe",
        &[
            (1000, 10, 90001, "cast"),
            (3000, 10, 90002, "cast"),
            (5000, 10, 90005, "cast"),
            (10000, 10, 90004, "cast"),
            (20000, 10, 90011, "cast"),
            (22000, 10, 90012, "cast"),
            (24000, 10, 90030, "cast"),
        ],
        26000,
        false,
    ));
    for log in &mut logs {
        log["report"]["fights"][0]["phaseTransitions"] =
            json!([{"id":1,"startTime":1000},{"id":2,"startTime":19000}]);
    }
    with_logs(&logs, |dir| {
        let output = dir.join("isolated.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        assert_eq!(report["extensions"]["unresolvedPhaseIds"], json!([2]));
        assert!(
            !report["extensions"]["branches"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(
            report["slots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|slot| slot["abilityIds"] == json!([90030])
                    && slot["evidence"] == "unresolvedPath")
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[rstest]
#[case::all_dead(2, false, Some(3100))]
#[case::one_alive(1, false, None)]
#[case::resurrected(2, true, None)]
fn raid_party_defeat_needs_every_participant_and_respects_resurrection(
    #[case] deaths: usize,
    #[case] resurrect: bool,
    #[case] expected: Option<i64>,
) {
    let mut log = multi_log("party", &[(1000, 10, 90001, "cast")], 6000, false);
    log["report"]["fights"][0]["friendlyPlayers"] = json!([20, 21, 22]);
    log["report"]["masterData"]["actors"].as_array_mut().unwrap().extend([
        json!({"id":20,"name":"Tank","gameID":1000020,"type":"Player","subType":"Paladin"}),
        json!({"id":21,"name":"Healer","gameID":1000021,"type":"Player","subType":"WhiteMage"}),
        json!({"id":22,"name":"Limit Break","gameID":1000022,"type":"Player","subType":"LimitBreak"})]);
    for (at, id) in [(4000, 20), (4100, 21)].into_iter().take(deaths) {
        log["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":at,"type":"death","fight":2,"targetID":id}));
    }
    if resurrect {
        log["events"]
            .as_array_mut()
            .unwrap()
            .push(json!({"timestamp":4200,"type":"resurrect","fight":2,"targetID":21}));
    }
    log["collection"]["eventCount"] = json!(log["events"].as_array().unwrap().len());
    with_file(&log, |input| {
        assert_eq!(load_one(input).unwrap().pull.party_defeat_ms, expected);
    });
}

#[test]
fn raid_mixed_alias_packets_match_one_explicit_alternative_wave() {
    let mut logs: Vec<_> = [90002, 90012, 90013]
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            multi_log(
                &format!("full-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (1500, 11, id, "begincast"),
                    (2000, 10, 90007, "cast"),
                    (3000, 11, id, "cast"),
                    (6000, 10, 90005, "cast"),
                    (9000, 10, 90004, "cast"),
                ],
                10000,
                true,
            )
        })
        .collect();
    let mut partial = multi_log(
        "partial",
        &[
            (1000, 10, 90001, "cast"),
            (1500, 11, 90012, "begincast"),
            (1500, 11, 90013, "begincast"),
            (2000, 10, 90007, "cast"),
            (3000, 11, 90012, "cast"),
            (3000, 11, 90013, "cast"),
        ],
        4000,
        false,
    );
    for (index, instance) in [(1, 1), (2, 2), (4, 1), (5, 2)] {
        partial["events"][index]["sourceInstance"] = json!(instance);
    }
    logs.push(partial);
    for log in &mut logs {
        for ability in log["report"]["masterData"]["abilities"]
            .as_array_mut()
            .unwrap()
        {
            if [90002, 90012, 90013]
                .iter()
                .any(|id| ability["gameID"].as_i64() == Some(*id))
            {
                ability["name"] = json!("Choice");
            }
        }
    }
    with_logs(&logs, |dir| {
        let output = dir.join("mixed.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(
                coverage["omittedEventIndices"],
                json!([]),
                "{}",
                report["extensions"]
            );
        }
    });
}

#[test]
fn raid_reversed_copy_packets_retain_their_cast_starts() {
    let mut log = multi_log(
        "a",
        &[
            (1000, 10, 90001, "cast"),
            (3000, 10, 90002, "begincast"),
            (3001, 11, 90012, "begincast"),
            (4000, 11, 90012, "cast"),
            (4001, 10, 90002, "cast"),
            (5000, 10, 90002, "begincast"),
            (5001, 11, 90012, "begincast"),
            (6000, 11, 90012, "cast"),
            (6001, 10, 90002, "cast"),
            (8000, 10, 90004, "cast"),
        ],
        9000,
        true,
    );
    log["report"]["masterData"]["actors"][1]["name"] = json!("Boss");
    log["report"]["masterData"]["actors"][1]["subType"] = json!("Boss");
    for ability in log["report"]["masterData"]["abilities"]
        .as_array_mut()
        .unwrap()
    {
        if ability["gameID"] == 90012 {
            ability["name"] = json!("Ability 90002");
        }
    }
    let mut peer = log.clone();
    peer["report"]["code"] = json!("b");
    peer["collection"]["reportCode"] = json!("b");
    with_logs(&[log, peer], |dir| {
        let output = dir.join("copies.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        let casts: Vec<_> = report["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["abilityIds"] == json!([90002, 90012]))
            .collect();
        assert_eq!(casts.len(), 2);
        assert!(
            casts
                .iter()
                .all(|slot| slot["evidence"] != "conditionalInstantHelper")
        );
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
    });
}

#[test]
fn raid_instant_burst_choices_keep_their_early_discriminators() {
    let logs: Vec<_> = [(90002, 90005), (90003, 90006)]
        .into_iter()
        .enumerate()
        .map(|(i, (choice, tail))| {
            multi_log(
                &format!("burst-{i}"),
                &[
                    (1000, 10, 90001, "cast"),
                    (2000, 10, choice, "cast"),
                    (4000, 10, choice, "cast"),
                    (6000, 10, choice, "cast"),
                    (9000, 10, tail, "cast"),
                    (10000, 10, 90004, "cast"),
                ],
                11000,
                true,
            )
        })
        .collect();
    with_logs(&logs, |dir| {
        let output = dir.join("burst.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        let selectors: Vec<_> = report["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["selector"] == true)
            .collect();
        assert_eq!(selectors.len(), 2);
        assert!(selectors.iter().all(
            |slot| slot["abilityIds"] == json!([90002]) || slot["abilityIds"] == json!([90003])
        ));
    });
}

#[test]
fn raid_unstarted_boss_followups_keep_variable_packet_counts_as_observations() {
    let first = multi_log(
        "three",
        &[
            (500, 10, 90001, "begincast"),
            (1000, 10, 90001, "cast"),
            (3000, 10, 90002, "cast"),
            (5000, 10, 90002, "cast"),
            (7000, 10, 90002, "cast"),
            (10000, 10, 90004, "cast"),
        ],
        11000,
        true,
    );
    let second = multi_log(
        "two",
        &[
            (500, 10, 90001, "begincast"),
            (1000, 10, 90001, "cast"),
            (3000, 10, 90002, "cast"),
            (7000, 10, 90002, "cast"),
            (10000, 10, 90004, "cast"),
        ],
        11000,
        true,
    );
    with_logs(&[first, second], |dir| {
        let output = dir.join("followups.yaml");
        prepare(dir, &output, &[]).unwrap();
        let report: Value =
            serde_json::from_slice(&fs::read(output.with_extension("report.json")).unwrap())
                .unwrap();
        for coverage in report["outputCoverage"].as_array().unwrap() {
            assert_eq!(coverage["omittedEventIndices"], json!([]));
        }
        assert!(
            report["slots"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|slot| slot["abilityIds"] == json!([90002]))
                .all(|slot| slot["evidence"] == "conditionalInstantHelper")
        );
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
fn input_type_errors_include_the_nested_field_path() {
    let mut data = sample();
    data["events"][2]["timestamp"] = json!("invalid");
    with_file(&data, |path| {
        let error = inspect(&[path]).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains(&path.display().to_string()));
        assert!(message.contains("events[2].timestamp"), "{message}");
    });
}

#[test]
fn input_rejects_trailing_values_after_a_valid_log() {
    with_file(&sample(), |path| {
        let bytes = fs::read(path).unwrap();
        // Keep the full-input contract, e.g. a second JSON object cannot be silently ignored.
        for suffix in ["{}", "garbage"] {
            let mut invalid = bytes.clone();
            invalid.extend_from_slice(suffix.as_bytes());
            fs::write(path, invalid).unwrap();
            let error = inspect(&[path]).unwrap_err();
            assert!(format!("{error:#}").contains("trailing characters"));
        }
        let mut padded = bytes;
        padded.extend_from_slice(b" \n\t");
        fs::write(path, padded).unwrap();
        inspect(&[path]).unwrap();
    });
}

#[rstest]
#[case::not_an_array(json!({}))]
#[case::invalid_id(json!([{"id":"one","startTime":1000}]))]
#[case::before_fight(json!([{"id":1,"startTime":999}]))]
#[case::after_fight(json!([{"id":1,"startTime":2001}]))]
#[case::duplicate_clock(json!([{"id":1,"startTime":1000},{"id":2,"startTime":1000}]))]
#[case::reverse_clock(json!([{"id":1,"startTime":1500},{"id":2,"startTime":1400}]))]
fn input_rejects_malformed_phase_clocks(#[case] transitions: Value) {
    let mut data = sample();
    data["report"]["fights"][0]["phaseTransitions"] = transitions;
    with_file(&data, |path| {
        inspect(&[path]).unwrap_err();
    });
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
        assert!(events.iter().any(|e| e["sync"]["fields"]["id"] == "15F94"));
        // Successful dispatches use virtual clocks; the original C still occurred at 5s in both clears.
        let aligned = report.get("alignmentSlots").unwrap_or(&report["slots"]);
        let common = aligned
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["abilityIds"] == json!([90004]))
            .unwrap();
        assert_eq!(common["time"]["sampleCount"], 2);
        assert_eq!(common["absoluteTime"]["medianMs"], 5000.0);
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

// Keep cancellation local to one identity, e.g. restarting instance 1 must not discard instance 2.
#[rstest]
#[case::unknown_instance(None)]
#[case::known_instance(Some(1))]
fn cast_pairing_keeps_interleaved_identities_and_consumes_starts_once(
    #[case] instance: Option<i64>,
) {
    let mut data = multi_log(
        "interleaved",
        &[
            (100, 10, 90001, "begincast"),
            (110, 10, 90001, "begincast"),
            (120, 11, 90001, "begincast"),
            (130, 10, 90002, "begincast"),
            (140, 10, 90001, "begincast"),
            (150, 10, 90001, "cast"),
            (160, 11, 90001, "cast"),
            (170, 10, 90002, "cast"),
            (180, 10, 90001, "cast"),
            (190, 10, 90001, "cast"),
        ],
        1000,
        true,
    );
    for (event, instance) in data["events"].as_array_mut().unwrap().iter_mut().zip([
        instance,
        Some(2),
        instance,
        instance,
        instance,
        Some(2),
        instance,
        instance,
        instance,
        instance,
    ]) {
        event["sourceInstance"] = json!(instance);
    }
    with_file(&data, |path| {
        let groups = inspect(&[path]).unwrap();
        let rows = &groups[0].pulls[0].occurrences;
        assert_eq!(rows.len(), 10);
        assert_eq!(rows[0].completion_event_index, None);
        assert_eq!(rows[9].start_event_index, None);
        assert_eq!(rows[9].start_timestamp_ms, None);
        for (start, completion) in [(1, 5), (2, 6), (3, 7), (4, 8)] {
            assert_eq!(rows[start].completion_event_index, Some(completion));
            assert_eq!(rows[completion].start_event_index, Some(start));
            assert_eq!(
                rows[completion].start_timestamp_ms,
                Some(rows[start].timestamp_ms)
            );
        }
    });
}

// Equal timestamps retain source order, e.g. a completion before its start must stay unpaired.
#[rstest]
#[case::start_first(false)]
#[case::completion_first(true)]
fn cast_pairing_respects_source_order_at_equal_timestamps(#[case] completion_first: bool) {
    let mut data = multi_log(
        "equal-time",
        &[(100, 10, 90001, "begincast"), (100, 10, 90001, "cast")],
        1000,
        true,
    );
    if completion_first {
        data["events"].as_array_mut().unwrap().reverse();
    }
    with_file(&data, |path| {
        let groups = inspect(&[path]).unwrap();
        let rows = &groups[0].pulls[0].occurrences;
        assert_eq!(
            rows.iter().map(|row| row.event_index).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(
            rows[0].completion_event_index,
            (!completion_first).then_some(1)
        );
        assert_eq!(rows[1].start_event_index, (!completion_first).then_some(0));
        assert_eq!(
            rows[1].start_timestamp_ms,
            (!completion_first).then_some(1100)
        );
        assert_eq!(rows[0].start_event_index, None);
        assert_eq!(rows[1].completion_event_index, None);
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

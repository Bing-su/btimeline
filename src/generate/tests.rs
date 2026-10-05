#![allow(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap to fail at the source"
)]

use super::*;
use proptest::prelude::*;
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

fn multi_log(code: &str, rows: &[(i64, i64, i64, &str)], end: i64, kill: bool) -> Value {
    let mut data = sample();
    data["report"]["code"] = json!(code);
    data["collection"]["reportCode"] = json!(code);
    data["report"]["endTime"] = json!(1000 + end);
    data["report"]["fights"][0]["endTime"] = json!(1000 + end);
    data["report"]["fights"][0]["kill"] = json!(kill);
    data["collection"]["endTime"] = json!(1000 + end);
    data["collection"]["eventCount"] = json!(rows.len());
    data["report"]["masterData"]["abilities"] = json!(
        rows.iter()
            .map(|r| r.2)
            .collect::<BTreeSet<_>>()
            .iter()
            .map(|&id| json!({"gameID":id,"name":format!("Ability {id}"),"type":"1"}))
            .collect::<Vec<_>>()
    );
    data["events"] =
        json!(rows.iter().map(|&(at, actor, ability, kind)| json!({
        "timestamp":1000 + at,"type":kind,"sourceID":actor,"abilityGameID":ability,"fight":2
    })).collect::<Vec<_>>());
    data
}

fn with_logs(logs: &[Value], check: impl FnOnce(&Path)) {
    with_file(&sample(), |path| {
        let directory = path.with_extension("logs");
        fs::create_dir(&directory).unwrap();
        for (i, log) in logs.iter().enumerate() {
            fs::write(
                directory.join(format!("fight_{i}.json")),
                serde_json::to_vec(log).unwrap(),
            )
            .unwrap();
        }
        check(&directory);
        fs::remove_dir_all(directory).unwrap();
    });
}

fn draft_events(yaml: &str) -> Vec<Value> {
    let timeline: Value = serde_saphyr::from_str(yaml).unwrap();
    timeline["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "event")
        .cloned()
        .collect()
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
        let selected = multi::select_group(path, Some("Unseen Fight"), None, None).unwrap();
        let (yaml, report) = multi::build(selected, GenerateMode::Raid).unwrap();
        let events = draft_events(&yaml);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["at"], 1.5);
        assert_eq!(events[1]["at"], 2.1);
        assert_eq!(events[2]["at"], 4.5);
        assert_eq!(
            events[1]["sync"]["fields"]["id"],
            json!(["^15F92$", "^15F93$"])
        );
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
        let mut reversed = multi::select_group(path, None, None, None).unwrap();
        reversed.pulls.reverse();
        let (reversed_yaml, reversed_report) = multi::build(reversed, GenerateMode::Raid).unwrap();
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
fn dependent_paths_produce_common_draft_instead_of_independent_id_arrays(
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
                multi::select_group(path, None, None, None).unwrap(),
                GenerateMode::Raid,
            )
            .unwrap();
            let events = draft_events(&yaml);
            assert_eq!(events.len(), if intermediate { 3 } else { 2 });
            assert!(
                events
                    .iter()
                    .all(|event| event["sync"]["fields"]["id"].is_string())
            );
            assert_eq!(report["omittedSignals"].as_array().unwrap().len(), 2);
            assert!(
                report["outputCoverage"][1]["omittedEventIndices"]
                    .as_array()
                    .unwrap()
                    .len()
                    >= 2
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
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
        let error = multi::select_group(path, None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--name") && error.contains("Another New Encounter"));
        multi::select_group(path, Some("Missing"), None, None).unwrap_err();
        let selected = multi::select_group(path, Some("Unseen Fight"), None, None).unwrap();
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
        )
        .unwrap();
        assert_eq!(draft_events(&fs::read_to_string(output).unwrap()).len(), 1);
    });
    b["report"]["fights"][0]["name"] = json!("Unseen Fight");
    with_logs(
        &[multi_log("a", &[(1000, 10, 90001, "cast")], 5000, true), b],
        |path| {
            multi::select_group(path, Some("Unseen Fight"), None, None).unwrap_err();
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
        )
        .unwrap();
        assert_eq!(draft_events(&yaml)[1]["sync"]["enabled"], false);
        assert_eq!(
            report["syncConflicts"][0]["conflictingEvents"][0]["eventIndex"],
            3
        );
    });
}

#[test]
fn multi_dungeon_keeps_helper_only_inside_boss_spans() {
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
                multi::select_group(path, None, None, None).unwrap(),
                GenerateMode::Dungeon,
            )
            .unwrap();
            assert_eq!(draft_events(&yaml).len(), 3);
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
        )
        .unwrap();
        assert_eq!(
            draft_events(&yaml)
                .iter()
                .map(|e| e["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [2.0, 8.0, 12.0]
        );
        assert_eq!(report["slots"][1]["time"]["medianMs"], 4000.0);
        assert_eq!(report["blocks"][2]["time"]["medianMs"], 8000.0);
        assert_eq!(report["blocks"][3]["time"]["medianMs"], 12000.0);
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
        )
        .unwrap();
        assert_eq!(
            draft_events(&yaml)
                .iter()
                .map(|event| event["at"].as_f64().unwrap())
                .collect::<Vec<_>>(),
            [9.0, 11.0, 12.0]
        );
        assert_eq!(report["slots"][1]["time"]["medianMs"], 10000.0);
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
            let group = multi::select_group(path, None, None, None).unwrap();
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
            let (_, report) = multi::build(group, GenerateMode::Raid).unwrap();
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
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "btimeline-p2-{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&path, serde_json::to_vec(data).unwrap()).unwrap();
    check(&path);
    fs::remove_file(path).unwrap();
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
        let group = multi::select_group(path, None, None, None).unwrap();
        let comparison = alignment::compare(&group.pulls[0], &group.pulls[2]).unwrap();
        assert!(comparison.segments.iter().flat_map(|s| &s.slots).all(|s| {
            !matches!((&s.left,&s.right), (Some(a),Some(b)) if a.time_ms == 20000 && b.time_ms == 2000)
        }));
        let (yaml, report) = multi::build(group, GenerateMode::Raid).unwrap();
        let events = draft_events(&yaml);
        assert!(
            events
                .iter()
                .any(|e| e["at"] == 5.0 && e["sync"]["fields"]["id"] == "^15F94$")
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
            multi::select_group(path, None, None, None).unwrap(),
            GenerateMode::Raid,
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
        let error = multi::select_group(path, None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--encounter 9999 --difficulty 9"));
        assert!(error.contains("--encounter 123456 --difficulty 10"));
        multi::select_group(path, Some("Unseen Fight"), None, None).unwrap_err();
        multi::select_group(path, None, Some(123456), None).unwrap_err();
        let selected = multi::select_group(path, None, Some(123456), Some(10)).unwrap();
        assert_eq!(selected.key.encounter, 123456);
        assert_eq!(selected.key.difficulty, 10);
        assert_eq!(selected.pulls.len(), 1);
        multi::select_group(path, Some("Missing"), Some(123456), Some(10)).unwrap_err();
        let output = path.join("selected.yaml");
        draft::generate_selected(
            path,
            &output,
            GenerateMode::Raid,
            None,
            Some(123456),
            Some(10),
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
            .filter(|entry| entry["kind"] == "event")
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["at"], json!(0.2));
        assert_eq!(
            events[0]["sync"]["fields"]["source"],
            json!(r"^Helper \(A\)\+$")
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
            .filter(|entry| entry["kind"] == "event")
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
        assert!(
            generate(input, &output, GenerateMode::Dungeon)
                .unwrap_err()
                .to_string()
                .contains("No observed boss segment")
        );
        assert!(!output.exists());
    });
    with_file(&data, |input| {
        for (mode, expected) in [
            (GenerateMode::Raid, vec![0, 1, 2, 4, 5, 6, 8]),
            (GenerateMode::Dungeon, vec![1, 2, 5, 6]),
        ] {
            let output = input.with_extension(if mode == GenerateMode::Dungeon {
                "dungeon.yaml"
            } else {
                "raid.yaml"
            });
            generate(input, &output, mode).unwrap();
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

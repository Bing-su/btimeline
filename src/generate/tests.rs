#![allow(
    clippy::unwrap_used,
    reason = "test fixtures use unwrap to fail at the source"
)]

use super::*;
use proptest::prelude::*;
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

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

#[test]
fn rejects_conflicting_groups() {
    let mut changed = sample();
    changed["report"]["code"] = json!("other-report");
    changed["collection"]["reportCode"] = json!("other-report");
    changed["report"]["masterData"]["logVersion"] = json!(77);
    with_file(&sample(), |first| {
        with_file(&changed, |second| {
            assert!(
                inspect(&[first, second])
                    .unwrap_err()
                    .to_string()
                    .contains("Version conflict")
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

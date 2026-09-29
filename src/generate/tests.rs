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

use super::*;
use crate::fflogs::model::CollectedLog;
use crate::timeline::Timeline;

#[test]
fn target_windows_keep_reused_ids_in_their_own_mechanic() {
    let p: Profile = serde_json::from_value(json!({
        "version":1,"observations":"all_casts_and_starts","not_checked":["ACT"],
        "segments":[{"phase":"p","source":"Boss","entry":["100"],"completion":"fight_end",
            "targets":{"200":[
                {"name":"First","events":["first"],"window":{"anchor":"101","occurrence":1,"duration_ms":1000}},
                {"name":"Second","events":["second"],"window":{"anchor":"101","occurrence":2,"duration_ms":1000}}
            ]}}]
    })).unwrap();
    p.validate().unwrap();
    let input = log(
        1,
        &["Boss"],
        json!([
            cast(100, 1, 0x100),
            cast(200, 1, 0x101),
            cast(300, 1, 0x200),
            cast(1300, 1, 0x200),
            cast(2000, 1, 0x101),
            cast(2100, 1, 0x200)
        ]),
    );
    let projected = projection::project(&input, &p, None).unwrap();
    let ids: Vec<_> = projected.bosses[0]
        .targets
        .iter()
        .map(|target| target.event_ids.clone())
        .collect();
    assert_eq!(ids, [vec!["first"], vec!["second"]]);
    assert_eq!(
        projected.bosses[0]
            .audit
            .unclassified
            .iter()
            .find(|row| row.id == "200")
            .unwrap()
            .count,
        1
    );
}

#[test]
fn target_window_anchors_require_canonical_ability_ids() {
    for anchor in ["a01", "A01"] {
        let profile: Profile = serde_json::from_value(json!({
            "version":1,"observations":"all_casts","not_checked":[],
            "segments":[{"phase":"p","source":"Boss","entry":["100"],"completion":"fight_end",
                "targets":{"200":{"name":"Hit","events":["hit"],
                    "window":{"anchor":anchor,"occurrence":1,"duration_ms":1000}}}}]
        }))
        .unwrap();
        assert_eq!(profile.validate().is_ok(), anchor == "A01");
    }
}

#[test]
fn an_active_display_window_is_distinct_from_its_start() {
    let target = projection::Target {
        raw_index: 0,
        raw_indices: vec![0],
        at_ms: 2500,
        id: "100".into(),
        name: "Position".into(),
        lead_ms: None,
        event_ids: vec!["position".into()],
        equivalents: vec![],
        reference: projection::Reference::ActiveWindow,
    };
    assert_eq!(target.timing_error(2000, 1600), 0);
    assert_eq!(target.timing_error(2600, 1600), 100);
    assert_eq!(target.timing_error(900, 1600), 1);
}

#[test]
fn overlapping_context_rules_are_errors_instead_of_first_match_wins() {
    let p: Profile = serde_json::from_value(json!({
        "version":1,"observations":"all_casts","not_checked":["ACT"],
        "segments":[{"phase":"p","source":"Boss","entry":["100"],"completion":"fight_end",
            "targets":{"200":[
                {"name":"First","events":["first"]},
                {"name":"Second","events":["second"]}
            ]}}]
    }))
    .unwrap();
    p.validate().unwrap();
    let input = log(
        1,
        &["Boss"],
        json!([cast(100, 1, 0x100), cast(200, 1, 0x200)]),
    );
    assert!(
        format!(
            "{:#}",
            projection::project(&input, &p, None)
                .err()
                .expect("overlapping rules must fail")
        )
        .contains("Overlapping target windows")
    );
}

#[test]
fn explicit_groups_retain_all_raw_members_without_chaining_separate_waves() {
    let p: Profile = serde_json::from_value(json!({
        "version":1,"observations":"all_casts","not_checked":["ACT"],
        "segments":[{"phase":"p","source":"Boss","entry":["100"],"completion":"fight_end",
            "targets":{"200":{"name":"Wave","events":["wave"],"group":"wave","group_window_ms":1600}}}]
    })).unwrap();
    p.validate().unwrap();
    let input = log(
        1,
        &["Boss"],
        json!([
            cast(100, 1, 0x100),
            cast(200, 1, 0x200),
            cast(1700, 1, 0x200),
            cast(3200, 1, 0x200)
        ]),
    );
    let result = projection::project(&input, &p, None).unwrap();
    let members: Vec<_> = result.bosses[0]
        .targets
        .iter()
        .map(|target| target.raw_indices.clone())
        .collect();
    assert_eq!(members, [vec![1, 2], vec![3]]);
}

fn profile(name: &str) -> Profile {
    let text = match name {
        "mistwake" => include_str!("../../timelines/mistwake.evaluation.json"),
        "griffin" => include_str!("../../timelines/mistwake-griffin.evaluation.json"),
        "clyteum" => include_str!("../../timelines/clyteum.evaluation.json"),
        _ => include_str!("../../timelines/sandoria.evaluation.json"),
    };
    let profile: Profile = serde_json::from_str(text).expect("valid test fixture");
    profile.validate().expect("valid test fixture");
    profile
}

#[test]
fn griffin_keeps_control_noise_and_requires_the_observed_defeat() {
    let p = profile("griffin");
    let model =
        crate::timeline::parse(include_str!("../../timelines/mistwake-griffin.kdl")).unwrap();
    p.validate_bindings(&model).unwrap();
    let mut raw = log(
        4549,
        &["Thundergust Griffin", "Shock Storm", "Other"],
        json!([
            cast(0, 1, 0xb0eb),
            cast(100, 3, 7),
            cast(200, 1, 0xb0f4),
            cast(300, 2, 0xb0f4),
            cast(301, 2, 0xb0f4),
            health(400, 1, 0),
            cast(500, 1, 0xb0f9)
        ]),
    );
    let result = projection::project(&raw, &p, None).unwrap();
    assert_eq!(result.recording.observations.len(), 5);
    assert_eq!(result.bosses[0].end_ms, 400);
    assert_eq!(
        result.bosses[0]
            .targets
            .iter()
            .map(|target| target.raw_index)
            .collect::<Vec<_>>(),
        vec![0, 3]
    );
    raw.events.remove(5);
    assert!(
        projection::project(&raw, &p, None)
            .err()
            .unwrap()
            .downcast_ref::<InputExclusion>()
            .is_some()
    );
}
fn log(encounter: i64, names: &[&str], events: Value) -> CollectedLog {
    let mut raw: Value = serde_json::from_str(include_str!("../../tests/fixtures/pull.json"))
        .expect("valid test fixture");
    raw["report"]["masterData"]["actors"] = json!(
        names
            .iter()
            .enumerate()
            .map(
                |(i, name)| json!({"id":i+1,"name":name,"gameID":i+1,"type":"NPC","subType":"Boss"})
            )
            .collect::<Vec<_>>()
    );
    raw["report"]["fights"][0]["encounterID"] = json!(encounter);
    raw["report"]["fights"][0]["endTime"] = json!(260000);
    raw["events"] = events;
    serde_json::from_value(raw).expect("valid test fixture")
}
fn cast(at: i64, source: i64, id: i64) -> Value {
    json!({"timestamp":at,"type":"cast","sourceID":source,"abilityGameID":id})
}
fn health(at: i64, target: i64, hp: i64) -> Value {
    json!({"timestamp":at,"type":"damage","targetID":target,"targetResources":{"hitPoints":hp,"maxHitPoints":100}})
}

#[test]
fn mistwake_preserves_helper_targets_waves_and_observations() {
    let input = log(
        1,
        &[
            "Treno Catoblepas",
            "Amdusias",
            "Thundergust Griffin",
            "Shock Storm",
            "Player",
        ],
        json!([
            cast(0, 5, 0xa93f),
            cast(100, 1, 0xa93f),
            cast(200, 1, 0xa943),
            health(300, 1, 0),
            cast(400, 2, 0xb11d),
            cast(500, 2, 0xb11f),
            cast(501, 2, 0xb11f),
            health(600, 2, 0),
            cast(700, 3, 0xb0eb),
            cast(800, 4, 0xb0f4),
            health(900, 3, 0)
        ]),
    );
    let p = profile("mistwake");
    let result = projection::project(&input, &p, None).expect("valid test fixture");
    assert_eq!(result.recording.observations.len(), 8);
    assert_eq!(
        result
            .bosses
            .iter()
            .map(|b| (b.entry_ms, b.end_ms, b.targets.len()))
            .collect::<Vec<_>>(),
        vec![(100, 300, 2), (400, 600, 2), (700, 900, 2)]
    );
    assert_eq!(result.bosses[1].targets[1].raw_index, 5);
    assert_eq!(result.bosses[2].targets[1].id, "B0F4");
    let mut incomplete = input;
    incomplete.events.remove(7);
    assert!(
        projection::project(&incomplete, &p, None)
            .err()
            .unwrap()
            .downcast_ref::<InputExclusion>()
            .is_some()
    );
}

#[test]
fn clyteum_aliases_scoped_unknowns_and_resets_match_previous_projector() {
    let input = log(
        1,
        &["Eye of the Scorpion", "Chort", "Malphas", ""],
        json!([
            cast(0, 4, 1),
            cast(100, 1, 0xbf00),
            cast(200, 1, 0xc3ff),
            cast(201, 1, 0xc400),
            health(300, 1, 0),
            cast(350, 4, 1),
            cast(400, 2, 0xbef4),
            cast(500, 2, 0xbee5),
            health(600, 2, 0),
            cast(700, 3, 0xbf18),
            cast(800, 3, 0xc489),
            health(900, 3, 0)
        ]),
    );
    let p = profile("clyteum");
    let result = projection::project(&input, &p, None).expect("valid test fixture");
    assert_eq!(result.recording.observations.len(), 7);
    assert_eq!(result.bosses[0].targets[1].name, "Petrifying Beam");
    assert_eq!(result.bosses[0].targets.len(), 2);
    let mut broken = input;
    broken.events.insert(
        4,
        serde_json::from_value(cast(250, 4, 1)).expect("valid test fixture"),
    );
    assert!(
        projection::project(&broken, &p, None)
            .err()
            .unwrap()
            .to_string()
            .contains("Unknown cast source")
    );
    broken.events.remove(4);
    broken.events.insert(
        4,
        serde_json::from_value(health(250, 1, 70)).expect("valid test fixture"),
    );
    broken.events.insert(
        5,
        serde_json::from_value(health(260, 1, 95)).expect("valid test fixture"),
    );
    assert!(
        projection::project(&broken, &p, None)
            .err()
            .unwrap()
            .to_string()
            .contains("Possible attempt reset")
    );
}

#[test]
fn sandoria_keeps_simultaneous_targets_and_starts_and_bounds_the_joint_scope() {
    let mut input = log(
        2068,
        &["Omega, the One", "Ultima, the Feared"],
        json!([
            cast(0,1,0xad2b),{"timestamp":10000,"type":"begincast","sourceID":2,"abilityGameID":0xad08},cast(12000,2,0xad08),cast(12001,2,0xad08),cast(12002,1,0xb087),cast(240000,2,0xad09)
        ]),
    );
    let p = profile("sandoria");
    let result = projection::project(&input, &p, None).expect("valid test fixture");
    assert_eq!(result.bosses.len(), 1);
    assert_eq!(result.bosses[0].end_ms, 238000);
    assert_eq!(result.bosses[0].targets.len(), 3);
    assert_eq!(result.recording.observations[1].kind, "StartsUsing");
    input.events.insert(
        5,
        serde_json::from_value(health(15000, 1, 0)).expect("valid test fixture"),
    );
    let result = projection::project(&input, &p, None).expect("valid test fixture");
    assert_eq!(result.bosses[0].end_ms, 15000);
    assert!(
        !result
            .recording
            .observations
            .iter()
            .any(|o| o.kind == "WasDefeated")
    );
    let kirin = log(
        2067,
        &["Faithbound Kirin"],
        json!([
            cast(0, 1, 0xadca),
            cast(98000, 1, 0xb07b),
            cast(103100, 1, 0xb07c)
        ]),
    );
    let result = projection::project(&kirin, &p, None).expect("valid test fixture");
    assert_eq!(
        result.bosses[0]
            .targets
            .iter()
            .map(|t| t.lead_ms)
            .collect::<Vec<_>>(),
        vec![None, Some(2000), None]
    );
}

#[test]
fn empty_eligible_set_and_tool_errors_cannot_pass() {
    assert_eq!(summary(&[]).get("passed"), Some(&json!(false)));
    let excluded = json!({"status":"excluded","passed":null});
    assert_eq!(
        summary(std::slice::from_ref(&excluded)).get("passed"),
        Some(&json!(false))
    );
    assert_eq!(
        summary(&[
            excluded,
            json!({"status":"error","passed":false}),
            json!({"status":"evaluated","passed":true})
        ]),
        json!({"selected":3,"excluded":1,"eligible":2,"passed_traces":1,"passed":false})
    );
}

#[test]
fn future_observations_do_not_repair_a_five_second_forecast() {
    let p:Profile=serde_json::from_value(json!({"version":1,"observations":"scoped_casts","segments":[{"phase":"boss/main","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end","targets":{"B528":{"name":"Hit","events":["boss/main/first","boss/main/second"]}}}],"not_checked":[]})).expect("valid test fixture");
    let raw: CollectedLog = serde_json::from_str(include_str!("../../tests/fixtures/pull.json"))
        .expect("valid test fixture");
    let model: Timeline = crate::timeline::parse(
        r###"
timeline 2 "Test" {
    start "Ability" id="B528"
    phase boss {
        step main {
            enter "Ability" id="B528"
            event first 0.0 name="Hit"
            event second 10.0 name="Hit" {
                sync 10.0 "Ability" id="B528" before=3.0 after=1.0
            }
        }
    }
}
"###,
    )
    .expect("valid test fixture");
    let projected = projection::project(&raw, &p, None).expect("valid test fixture");
    let manifest:Manifest=serde_json::from_value(json!({"timeline":"unused","timeline_sha256":"unused","role":"test","forecast_lead_ms":5000,"horizon_ms":6500,"tolerance_ms":1500,"inputs":[]})).expect("valid test fixture");
    let result = score::score(&model, &projected, &manifest).expect("valid test fixture");
    // Its own future sync repairs execution at impact, but cannot retroactively repair the earlier warning.
    assert_eq!(result["bosses"][0]["targets"][2]["error_ms"], json!(0));
    assert_eq!(
        result["bosses"][0]["targets"][2]["forecast"]["passed"],
        json!(false)
    );
    assert_eq!(result["bosses"][0]["targets"][1]["passed"], json!(false));
}

#[test]
fn edited_evaluation_sources_are_rejected() {
    let directory = tempfile::tempdir().expect("valid test fixture");
    let path = directory.path().join("candidate.kdl");
    fs::write(&path, b"frozen").expect("valid test fixture");
    ensure_unchanged(path.to_str().expect("valid test fixture"), b"frozen")
        .expect("valid test fixture");
    fs::write(&path, b"edited").expect("valid test fixture");
    assert!(ensure_unchanged(path.to_str().unwrap(), b"frozen").is_err());
}

#[test]
fn projection_does_not_execute_hypothetical_branch_closures() {
    let p: Profile = serde_json::from_value(json!({"version":1,"observations":"scoped_casts","segments":[{"phase":"boss/main","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end","targets":{"B528":{"name":"Hit","events":["boss/main/first","boss/main/second"]}}}],"not_checked":[]})).expect("profile");
    let raw: CollectedLog =
        serde_json::from_str(include_str!("../../tests/fixtures/pull.json")).expect("fixture");
    let model = crate::timeline::parse(
        r###"
timeline 2 "Test" {
    start "Ability" id="B528"
    phase boss {
        step main {
            enter "Ability" id="B528"
            event first 0.0 name="Hit"
            event second 8.0 name="Hit"
            branch choice open=1.0 close=4.0 {
                option left {
                    when "Left"
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
    .expect("timeline");
    let projected = projection::project(&raw, &p, None).expect("projection");
    let manifest: Manifest = serde_json::from_value(json!({"timeline":"unused","timeline_sha256":"unused","role":"test","forecast_lead_ms":5000,"horizon_ms":6500,"tolerance_ms":1500,"inputs":[]})).expect("manifest");
    let result = score::score(&model, &projected, &manifest).expect("score");
    let target = &result["bosses"][0]["targets"][2];
    assert_eq!(target["forecast"]["current_diagnostics"], json!([]));
    assert_eq!(target["forecast"]["projection_diagnostics"], json!([]));
    assert_eq!(target["failure_reasons"], json!([]));
    assert_eq!(target["forecast"]["lead_ms"], 5000);
}

#[test]
fn a_same_name_future_bar_cannot_stand_in_for_a_different_event_identity() {
    let p: Profile = serde_json::from_value(json!({"version":1,"observations":"scoped_casts","segments":[{"phase":"boss/main","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end","targets":{"B528":{"name":"Label is not a matcher","events":["boss/main/opening","boss/main/other","boss/main/repair"]}}}],"not_checked":[]})).expect("profile");
    let raw = log(
        1,
        &["Synthetic Boss"],
        json!([
            cast(0, 1, 0xb528),
            cast(8000, 1, 0xb528),
            cast(8000, 1, 0xb529)
        ]),
    );
    let model = crate::timeline::parse(
        r###"
timeline 2 "Test" {
    start "Ability" id="B528"
    phase boss {
        step main {
            enter "Ability" id="B528"
            event opening 0.0 name="Hit"
            event other 8.0 name="Hit"
            event repair 12.0 name="Hit" {
                sync 12.0 "Ability" id="B529" before=5.0 after=1.0
            }
        }
    }
}
"###,
    )
    .expect("timeline");
    p.validate_bindings(&model).expect("bindings");
    let projected = projection::project(&raw, &p, None).expect("projection");
    let manifest: Manifest = serde_json::from_value(json!({"timeline":"unused","timeline_sha256":"unused","role":"test","forecast_lead_ms":5000,"horizon_ms":6500,"tolerance_ms":1500,"inputs":[]})).expect("manifest");
    let result = score::score(&model, &projected, &manifest).expect("score");
    let target = &result["bosses"][0]["targets"][1];
    assert_eq!(target["matched_event"]["event"]["id"], "boss/main/repair");
    assert_eq!(target["error_ms"], 0);
    assert_eq!(target["forecast"]["passed"], false);
}

#[test]
fn option_to_merge_replacement_requires_an_explicit_equivalence_group() {
    let raw = log(
        1,
        &["Synthetic Boss"],
        json!([
            cast(0, 1, 0xb528),
            cast(1000, 1, 0xb529),
            cast(7000, 1, 0xb52a),
            cast(10000, 1, 0xb528)
        ]),
    );
    let model = crate::timeline::parse(
        r###"
timeline 2 "Test" {
    start "Ability" id="B528"
    phase boss {
        step main {
            enter "Ability" id="B528"
            event opening 0.0 name="Hit"
            branch choice open=0.0 close=3.0 {
                option left {
                    when "Ability" id="B529"
                    event option 9.0 name="Hit"
                }
                option right {
                    when "Ability" id="B52B"
                }
                merge {
                    when "Ability" id="B52A" at=7.0 before=1.0 after=1.0
                    event common 3.0 name="Hit"
                }
            }
        }
    }
}
"###,
    )
    .expect("timeline");
    for handoff in [false, true] {
        let mut config = json!({"version":1,"observations":"scoped_casts","segments":[{"phase":"boss/main","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end","targets":{"B528":{"name":"Hit","events":["boss/main/opening","boss/main/choice/left/option","boss/main/choice/merge/common"]}}}],"not_checked":[]});
        if handoff {
            config["segments"][0]["targets"]["B528"]["equivalents"] = json!([[
                "boss/main/choice/left/option",
                "boss/main/choice/merge/common"
            ]]);
        }
        let p: Profile = serde_json::from_value(config).expect("profile");
        p.validate_bindings(&model).expect("bindings");
        let projected = projection::project(&raw, &p, None).expect("projection");
        let manifest: Manifest = serde_json::from_value(json!({"timeline":"unused","timeline_sha256":"unused","role":"test","forecast_lead_ms":5000,"horizon_ms":6500,"tolerance_ms":1500,"inputs":[]})).expect("manifest");
        let result = score::score(&model, &projected, &manifest).expect("score");
        let target = &result["bosses"][0]["targets"][1];
        assert_eq!(
            target["matched_event"]["event"]["id"],
            "boss/main/choice/merge/common"
        );
        assert_eq!(target["forecast"]["passed"], handoff);
        if handoff {
            assert_eq!(
                target["forecast"]["matched_event"]["id"],
                "boss/main/choice/left/option"
            );
        }
    }
}

#[test]
fn bindings_reject_unknown_hidden_foreign_and_overlapping_definitions() {
    let model = crate::timeline::parse(
        r###"
timeline 2 "Bindings" {
    phase boss {
        step main {
            enter "Begin"
            event a 0.0 name="A"
            event b 1.0 name="B"
            event hidden 2.0 name="Control" visible=#false
        }
    }
    phase other {
        step main {
            enter "Other"
            event foreign 0.0 name="A"
        }
    }
}
"###,
    )
    .unwrap();
    for targets in [
        json!({"A001":{"name":"A","events":["unknown"]}}),
        json!({"A001":{"name":"A","events":["boss/main/hidden"]}}),
        json!({"A001":{"name":"A","events":["other/main/foreign"]}}),
        json!({"A001":{"name":"A","events":["boss/main/a","boss/main/b"]},"A002":{"name":"A","events":["boss/main/b"]}}),
        json!({"A001":{"name":"A","events":["boss/main/a"],"group":"wave"},"A002":{"name":"A","events":["boss/main/b"],"group":"wave"}}),
        json!({"A001":{"name":"A","events":["boss/main/a","boss/main/b"],"equivalents":[["boss/main/a","other/main/foreign"]]}}),
    ] {
        let p: Profile = serde_json::from_value(json!({"version":1,"observations":"scoped_casts","segments":[{"phase":"boss/main","source":"Boss","entry":["A001"],"completion":"fight_end","targets":targets}],"not_checked":[]})).unwrap();
        assert!(
            p.validate()
                .and_then(|()| p.validate_bindings(&model))
                .is_err()
        );
    }
}

#[test]
fn sandoria_scores_variant_casts_and_helper_bury_without_counting_duplicate_arms() {
    let p = profile("sandoria");
    let raw = log(
        2067,
        &["Faithbound Kirin", "Chiseled Arm"],
        json!([
            cast(0, 1, 0xadca),
            cast(163100, 1, 0xadaf),
            cast(168400, 1, 0xad9f),
            cast(173500, 1, 0xada4),
            cast(220000, 2, 0xadbf),
            cast(220000, 2, 0xadbf)
        ]),
    );
    let result = projection::project(&raw, &p, None).unwrap();
    assert_eq!(
        result.bosses[0]
            .targets
            .iter()
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>(),
        vec!["ADCA", "ADAF", "AD9F", "ADA4", "ADBF"]
    );
    let raw = log(
        2070,
        &["Eald'narche"],
        json!([cast(0, 1, 0xad51), cast(13100, 1, 0xad4c)]),
    );
    let result = projection::project(&raw, &p, None).unwrap();
    assert_eq!(result.bosses[0].targets.len(), 2);
    assert_eq!(result.bosses[0].targets[1].id, "AD4C");
}

#[rstest::rstest]
#[case(3500, false)]
#[case(2500, true)]
fn a_restarted_sequence_requires_its_own_forecast(#[case] lead_ms: i64, #[case] passed: bool) {
    let profile: Profile = serde_json::from_value(json!({
        "version":1,"observations":"scoped_casts","not_checked":[],
        "segments":[{"phase":"boss/main","source":"Synthetic Boss","entry":["B528"],"completion":"fight_end",
        "targets":{"B529":{"name":"Hit","events":["boss/main/reaction/reaction/hit"]}}}]
    })).unwrap();
    let raw = log(
        1,
        &["Synthetic Boss"],
        json!([
            cast(0, 1, 0xb528),
            cast(1000, 1, 0xb530),
            cast(2000, 1, 0xb530),
            cast(5000, 1, 0xb529)
        ]),
    );
    let model = crate::timeline::parse(
        r###"
timeline 2 "Test" {
    start "Ability" id="B528"
    phase boss {
        step main {
            enter "Ability" id="B528"
            reaction reaction {
                on "Ability" id="B530"
                event hit 3.0 name="Hit"
            }
        }
    }
}
"###,
    )
    .unwrap();
    profile.validate_bindings(&model).unwrap();
    let projected = projection::project(&raw, &profile, None).unwrap();
    let manifest: Manifest = serde_json::from_value(json!({
        "timeline":"unused","timeline_sha256":"unused","role":"test","forecast_lead_ms":lead_ms,
        "horizon_ms":6500,"tolerance_ms":1500,"inputs":[]
    }))
    .unwrap();
    let result = score::score(&model, &projected, &manifest).unwrap();
    let target = &result["bosses"][0]["targets"][0];
    assert_eq!(target["matched_event"]["context"]["sequence_instance"], 2);
    assert_eq!(target["forecast"]["passed"], passed);
    assert_eq!(
        target["forecast"]["matched_event"]["context"]["sequence_instance"],
        if passed { json!(2) } else { Value::Null }
    );
}

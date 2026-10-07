use proptest::prelude::*;
use rstest::rstest;

use super::*;

const SOURCE: &str = r#"# yaml-language-server: $schema=https://raw.githubusercontent.com/Bing-su/btimeline/main/schema/btimeline-v1.schema.json
schemaVersion: 1
hideNames: ["--sync--"]
entries:
  - kind: generatorOptions
    targetable: ["Black Cat"]
    ignoredCombatants: ["Helper"]
    phaseStarts: [{ abilityId: "9441", at: 10 }]
  - kind: note
    text: "Opening"
  - kind: event
    at: 0.0
    name: "--sync--"
    sync: { log: InCombat, fields: { inGameCombat: "1" }, window: [0, 1] }
  - kind: label
    at: 100.0
    name: repeat
  - kind: syncOrder
    enabled: false
  - kind: event
    at: 145.0
    name: "Mouser"
    duration: 9.7
    sync: { log: Ability, fields: { id: "9441", source: "Black Cat" } }
    jump: { to: repeat, when: always }
    note: "hit"
  - kind: event
    at: 150.0
    name: "Mouser follow-up"
    sync: { enabled: false, regex: "9442", window: [1, 2] }
    note: "not a sync"
  - kind: syncOrder
    enabled: true
  - kind: abilityCatalog
    phase: "Phase 1"
    abilities:
      - { id: "9441", name: "Mouser" }
      - { id: "9442", name: "Mouser follow-up", note: "Extra hit", ignored: true }
"#;

#[test]
fn exported_schema_rejects_field_constraints() {
    use serde_json::json;
    let schema = generated_schema().expect("generated schema");
    assert_eq!(
        schema.pointer("/properties/resetOn/default"),
        Some(&json!(["wipe"]))
    );
    assert_eq!(
        schema.pointer("/properties/hideNames/default"),
        Some(&json!(["--Reset--", "--sync--"]))
    );
    let entry = |entry: Value| json!({"schemaVersion": 1, "entries": [entry]});
    jsonschema::validate(&schema, &entry(json!({"kind": "note", "text": "ok"}))).unwrap();
    for at in [0.1, 1.3, 145.1, 6553.5] {
        assert!(
            jsonschema::validate(
                &schema,
                &entry(json!({"kind": "event", "at": at, "name": "ok"}))
            )
            .is_ok(),
            "schema rejected {at}"
        );
    }
    for invalid in [
        json!({"schemaVersion": 1, "resetOn": ["wipe", "wipe"], "entries": []}),
        json!({"schemaVersion": 1, "hideNames": ["same", "same"], "entries": []}),
        entry(json!({"kind": "event", "at": 1.01, "name": "name"})),
        entry(json!({"kind": "event", "at": -1, "name": "name"})),
        entry(json!({"kind": "event", "at": 1, "name": "bad\"name"})),
        entry(json!({"kind": "event", "at": 1, "name": "bad\n"})),
        entry(json!({"kind": "event", "at": 1, "name": "name", "duration": 0})),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "jump": {"to": -1, "when": "sync"}}),
        ),
        entry(json!({"kind": "generatorOptions"})),
        entry(json!({"kind": "generatorOptions", "targetable": []})),
        entry(json!({"kind": "generatorOptions", "ignoredCombatants": []})),
        entry(json!({"kind": "generatorOptions", "phaseStarts": []})),
        entry(json!({"kind": "generatorOptions", "targetable": ["bad\"name"]})),
        entry(json!({"kind": "abilityCatalog", "phase": "bad\nphase", "abilities": []})),
        entry(
            json!({"kind": "abilityCatalog", "abilities": [{"id": "AB", "name": "name", "note": "bad\nnote"}]}),
        ),
        entry(json!({"kind": "event", "at": 1, "name": "name", "note": "bad\rnote"})),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "jump": {"to": 1.01, "when": "sync"}}),
        ),
        entry(json!({"kind": "abilityCatalog", "abilities": [{"id": "bad", "name": "name"}]})),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "Unknown", "fields": {"id": "A"}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "Ability", "fields": {}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "Ability", "fields": {"capture": "x"}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "InCombat", "fields": {"id": "A"}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "Ability", "fields": {"id": []}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"log": "Ability", "fields": {"id": "#"}}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"regex": "a", "window": [1.01, 2]}}),
        ),
        entry(
            json!({"kind": "event", "at": 1, "name": "name", "sync": {"regex": "a", "window": [-1, 2]}}),
        ),
    ] {
        assert!(
            jsonschema::validate(&schema, &invalid).is_err(),
            "schema accepted {invalid}"
        );
    }
}

#[test]
fn spec_render() {
    insta::assert_snapshot!(convert(SOURCE).expect("valid fixture"));
    convert(&SOURCE.replace("    phase: \"Phase 1\"\n", "")).unwrap();
    assert!(
        convert(&SOURCE.replace("duration: 9.7", "duration: 9.123"))
            .expect("arbitrary duration precision")
            .contains("duration 9.123")
    );
}

// Preserve hiding defaults and overrides across conversion and serialization, e.g. [] shows all rows.
#[test]
fn hide_names_defaults_and_overrides_round_trip() {
    for (setting, expected) in [
        ("", "hideall \"--Reset--\"\nhideall \"--sync--\"\n"),
        (
            "hideNames: [\"--Reset--\", \"--sync--\"]\n",
            "hideall \"--Reset--\"\nhideall \"--sync--\"\n",
        ),
        ("hideNames: []\n", ""),
        ("hideNames: [Hidden]\n", "hideall \"Hidden\"\n"),
    ] {
        let source = format!("schemaVersion: 1\nresetOn: []\n{setting}entries: []\n");
        assert_eq!(convert(&source).unwrap(), expected);
        let serialized = serde_saphyr::to_string(&parse(&source).unwrap()).unwrap();
        assert_eq!(convert(&serialized).unwrap(), expected);
    }
}

// Preserve lifecycle resets before directives, e.g. hideNames must not displace the first line.
#[test]
fn reset_lines_precede_timeline_content() {
    let wipe = "0.0 \"--Reset--\" ActorControl { command: \"4000000F\" } window 0,1000000 jump 0\n";
    let clear = "0.0 \"--Reset--\" SystemLogMessage { id: \"7DE\" } window 0,1000000 jump 0\n";
    for (setting, expected) in [
        ("", wipe.to_owned()),
        ("resetOn: [wipe]\n", wipe.to_owned()),
        ("resetOn: [wipe, areaClear]\n", format!("{wipe}{clear}")),
        ("resetOn: [areaClear, wipe]\n", format!("{wipe}{clear}")),
        ("resetOn: [areaClear]\n", clear.to_owned()),
        ("resetOn: []\n", String::new()),
    ] {
        let source = format!("schemaVersion: 1\n{setting}hideNames: [Hidden]\nentries: []\n");
        assert_eq!(
            convert(&source).unwrap(),
            format!("{expected}hideall \"Hidden\"\n")
        );
        // Preserve reset choices through serialization, e.g. [] cannot turn back into [wipe].
        let timeline = parse(&source).unwrap();
        let serialized = serde_saphyr::to_string(&timeline).unwrap();
        assert_eq!(convert(&serialized).unwrap(), convert(&source).unwrap());
        assert_eq!(
            serialized.contains("resetOn:"),
            timeline.reset_on != [ResetEvent::Wipe]
        );
    }
}

#[rstest]
#[case::unknown_reset("schemaVersion: 1", "schemaVersion: 1\nresetOn: [unknown]")]
#[case::duplicate_reset("schemaVersion: 1", "schemaVersion: 1\nresetOn: [wipe, wipe]")]
#[case::null_reset("schemaVersion: 1", "schemaVersion: 1\nresetOn: null")]
#[case::scalar_reset("schemaVersion: 1", "schemaVersion: 1\nresetOn: wipe")]
#[case::version("schemaVersion: 1", "schemaVersion: 2")]
#[case::duplicate_key("schemaVersion: 1", "schemaVersion: 1\nschemaVersion: 1")]
#[case::unknown_field("name: repeat", "name: repeat\n    bogus: true")]
#[case::event_precision("at: 145.0", "at: 145.01")]
#[case::invalid_ability_id("abilityId: \"9441\"", "abilityId: \"bad\"")]
#[case::phase_precision("abilityId: \"9441\", at: 10", "abilityId: \"9441\", at: 10.01")]
#[case::invalid_name("targetable: [\"Black Cat\"]", "targetable: [\"Bad\\\"Name\"]")]
#[case::null_duration("duration: 9.7", "duration: null")]
#[case::null_phase("phase: \"Phase 1\"", "phase: null")]
#[case::control_character("name: \"Mouser\"", "name: \"Mouser\\rBad\"")]
#[case::missing_label("to: repeat", "to: missing")]
#[case::invalid_sync_field("id: \"9441\", source", "capture: \"x\", source")]
#[case::invalid_sync_value("id: \"9441\", source", "id: \"#\", source")]
#[case::sync_order(
    "enabled: true\n  - kind: abilityCatalog",
    "enabled: false\n  - kind: abilityCatalog"
)]
fn rejects_invalid_source(#[case] from: &str, #[case] to: &str) {
    let invalid = SOURCE.replace(from, to);
    assert!(
        convert(&invalid).is_err(),
        "accepted invalid YAML: {invalid}"
    );
}

#[test]
fn rejects_multiple_documents() {
    let invalid = format!("{SOURCE}\n---\nschemaVersion: 1\nentries: []");
    convert(&invalid).unwrap_err();
}

#[test]
fn exported_schema_matches_types() {
    let generated = generated_schema().expect("serializable schema");
    let exported: Value =
        serde_json::from_str(include_str!("../../schema/btimeline-v1.schema.json"))
            .expect("valid exported schema");
    assert_eq!(generated, exported);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn valid_tenths_pass_field_validation(tenths in any::<u16>()) {
        prop_assert!(one_decimal(f64::from(tenths) / 10.0));
    }

    #[test]
    fn nonzero_hundredths_fail_field_validation(tenths in any::<u8>(), hundredth in 1u8..10) {
        let value = f64::from(tenths) / 10.0 + f64::from(hundredth) / 100.0;
        prop_assert!(!one_decimal(value));
    }

    #[test]
    fn tenths_render_without_rounding(seconds in 0u16..1000, tenth in 0u8..10) {
        let source = format!("schemaVersion: 1
entries:
  - kind: event
    at: {seconds}.{tenth}
    name: Tick
");
        let rendered = convert(&source).expect("valid tenth must convert");
        prop_assert_eq!(rendered.lines().last().unwrap(), format!(r#"{seconds}.{tenth} "Tick""#));
    }

    #[test]
    fn hundredths_are_rejected(seconds in 0u16..1000, tenth in 0u8..10, hundredth in 1u8..10) {
        let source = format!("schemaVersion: 1
entries:
  - kind: event
    at: {seconds}.{tenth}{hundredth}
    name: Tick
");
        prop_assert!(convert(&source).is_err());
    }
}

#[test]
fn network_field_regex_preserves_backslashes() {
    let source = SOURCE.replace("id: \"9441\", source", r"id: '\d+', source");
    let rendered = convert(&source).expect("valid network field regex");
    assert!(rendered.contains(r#"id: "\\d+""#), "{rendered}");
    let array_source = SOURCE.replace("id: \"9441\", source", r"id: ['\d+', '\w+'], source");
    let array_rendered = convert(&array_source).expect("valid regex alternatives");
    assert!(
        array_rendered.contains(r#"id: ["\\d+", "\\w+"]"#),
        "{array_rendered}"
    );
}

#[test]
fn validates_javascript_regex_syntax() {
    let source = "schemaVersion: 1\nentries:\n  - kind: event\n    at: 0\n    name: Test\n    sync: { regex: '\\Afoo' }\n";
    convert(source).unwrap();
    convert(&source.replace(r"\Afoo", "(?P<name>foo)")).unwrap_err();
}

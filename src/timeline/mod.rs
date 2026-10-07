use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use garde::Validate;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod render;
pub(crate) mod replay;
mod validation;

#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Timeline {
    #[schemars(extend("const" = 1))]
    pub schema_version: u8,
    // Select lifecycle resets, e.g. [areaClear] between dungeon bosses or [] to disable resets.
    #[serde(default = "default_reset_on", skip_serializing_if = "default_resets")]
    #[schemars(extend("default" = ["wipe"], "uniqueItems" = true))]
    pub reset_on: Vec<ResetEvent>,
    // Keep explicit overrides when serializing, e.g. [] disables hiding both lifecycle names.
    #[serde(default = "default_hide_names")]
    #[schemars(extend("default" = ["--Reset--", "--sync--"]))]
    #[garde(inner(pattern(r#"^[^"\r\n]+$"#)))]
    #[schemars(extend("uniqueItems" = true))]
    pub hide_names: Vec<String>,
    #[garde(dive)]
    pub entries: Vec<Entry>,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ResetEvent {
    Wipe,
    AreaClear,
}

fn default_reset_on() -> Vec<ResetEvent> {
    vec![ResetEvent::Wipe]
}

// Share hidden lifecycle names with generated drafts, e.g. omit reset and combat-start rows from bars.
pub(crate) fn default_hide_names() -> Vec<String> {
    vec!["--Reset--".into(), "--sync--".into()]
}

// Omit only the default, e.g. [] must stay explicit so a disabled reset survives serialization.
fn default_resets(reset_on: &[ResetEvent]) -> bool {
    reset_on == [ResetEvent::Wipe]
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Entry {
    Event {
        #[garde(range(min = 0.0), custom(garde_time))]
        #[schemars(extend("multipleOf" = 0.1))]
        at: f64,
        #[garde(pattern(r#"^[^"\r\n]+$"#))]
        name: String,
        #[garde(inner(custom(garde_duration)))]
        #[schemars(extend("exclusiveMinimum" = 0))]
        #[serde(skip_serializing_if = "Option::is_none")]
        duration: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sync: Option<Sync>,
        #[serde(skip_serializing_if = "Option::is_none")]
        jump: Option<Jump>,
        #[garde(inner(pattern(r"^[^\r]+$")))]
        #[schemars(pattern(r"^[^\r]+$"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Label {
        #[garde(range(min = 0.0), custom(garde_time))]
        #[schemars(extend("multipleOf" = 0.1))]
        at: f64,
        #[garde(pattern(r#"^[^"\r\n]+$"#))]
        name: String,
    },
    Note {
        #[garde(pattern(r"^[^\r]+$"))]
        text: String,
    },
    #[schemars(extend("anyOf" = [{"required": ["targetable"]}, {"required": ["ignoredCombatants"]}, {"required": ["phaseStarts"]}]))]
    GeneratorOptions {
        #[garde(inner(inner(pattern(r#"^[^"\r\n]+$"#))))]
        #[schemars(length(min = 1), inner(pattern(r#"^[^"\r\n]+$"#)))]
        targetable: Option<Vec<String>>,
        #[garde(inner(inner(pattern(r#"^[^"\r\n]+$"#))))]
        #[schemars(length(min = 1), inner(pattern(r#"^[^"\r\n]+$"#)))]
        ignored_combatants: Option<Vec<String>>,
        #[garde(dive)]
        #[schemars(length(min = 1))]
        phase_starts: Option<Vec<PhaseStart>>,
    },
    SyncOrder {
        enabled: bool,
    },
    AbilityCatalog {
        #[garde(dive)]
        abilities: Vec<Ability>,
        #[garde(inner(pattern(r"^[^\r\n]+$")))]
        #[schemars(pattern(r"^[^\r\n]+$"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        phase: Option<String>,
    },
}

impl Entry {
    // Recognize only the generated lifecycle row, e.g. other InCombat conditions still require evidence.
    pub(crate) fn is_combat_start(&self) -> bool {
        matches!(self, Self::Event {
            at: 0.0, duration: None, jump: None,
            sync: Some(Sync::Network(NetworkSync {
                log: LogType::InCombat, fields, enabled: true, window: Some([0.0, 1.0]),
            })), ..
        } if fields.len() == 1 && matches!(fields.get("inGameCombat"), Some(FieldPattern::One(value)) if value == "1"))
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum Sync {
    Network(NetworkSync),
    Regex(RegexSync),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub(crate) enum LogType {
    Ability,
    StartsUsing,
    InCombat,
    GainsEffect,
    LosesEffect,
    AddedCombatant,
    RemovedCombatant,
    Tether,
    HeadMarker,
    GameLog,
    Map,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NetworkSync {
    pub log: LogType,
    #[schemars(extend("minProperties" = 1))]
    pub fields: BTreeMap<String, FieldPattern>,
    #[serde(default = "yes", skip_serializing_if = "enabled_by_default")]
    #[schemars(extend("default" = true))]
    pub enabled: bool,
    #[schemars(extend("items" = {"type": "number", "minimum": 0, "multipleOf": 0.1}))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegexSync {
    #[schemars(pattern(r#"^[^#"\r\n/]*$"#))]
    regex: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[schemars(extend("items" = {"type": "number", "minimum": 0, "multipleOf": 0.1}))]
    window: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum FieldPattern {
    One(#[schemars(pattern(r#"^[^#"\r\n]*$"#))] String),
    Many(#[schemars(length(min = 1), inner(pattern(r#"^[^#"\r\n]*$"#)))] Vec<String>),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Jump {
    pub to: Destination,
    pub when: JumpWhen,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub(crate) enum Destination {
    Label(String),
    Time(#[schemars(range(min = 0.0), extend("multipleOf" = 0.1))] f64),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum JumpWhen {
    Sync,
    Always,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PhaseStart {
    #[garde(pattern(r"^[0-9A-F]+$"))]
    ability_id: String,
    #[garde(range(min = 0.0), custom(garde_time))]
    #[schemars(extend("multipleOf" = 0.1))]
    at: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ability {
    #[garde(pattern(r"^[0-9A-F]+$"))]
    pub id: String,
    #[garde(pattern(r"^[^\r\n]+$"))]
    pub name: String,
    #[garde(inner(pattern(r"^[^\r\n]+$")))]
    #[schemars(pattern(r"^[^\r\n]+$"))]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[schemars(extend("default" = false))]
    pub ignored: bool,
}

fn yes() -> bool {
    true
}

// Omit default sync settings in drafts, e.g. an active sync needs no explicit enabled: true.
fn enabled_by_default(enabled: &bool) -> bool {
    *enabled
}

pub fn export_schema(path: impl AsRef<Path>) -> Result<()> {
    let schema = generated_schema()?;
    fs::write(
        path.as_ref(),
        format!("{}\n", serde_json::to_string_pretty(&schema)?),
    )?;
    Ok(())
}

fn parse(source: &str) -> Result<Timeline> {
    let options = serde_saphyr::options! {
        strict_booleans: true,
        merge_keys: serde_saphyr::MergeKeyPolicy::Error,
    };
    let value: Value =
        serde_saphyr::from_str_with_options(source, options).context("Invalid YAML")?;
    from_value(value)
}

fn from_value(value: Value) -> Result<Timeline> {
    let schema = generated_schema()?;
    jsonschema::validate(&schema, &value).map_err(|e| anyhow::anyhow!("JSON Schema: {e}"))?;
    let timeline: Timeline = serde_json::from_value(value)?;
    timeline.validate().context("Semantic field validation")?;
    timeline.validate_relations()?;
    Ok(timeline)
}

// Validate generated entries through the same rules, e.g. an omitted pull still cannot hide an invalid name.
pub(crate) fn validate_value(value: Value) -> Result<()> {
    from_value(value)?;
    Ok(())
}

pub fn validate_file(path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    parse(&fs::read_to_string(path).with_context(|| format!("Reading {}", path.display()))?)?;
    Ok(())
}

pub fn convert_file(input: impl AsRef<Path>, output: impl AsRef<Path>) -> Result<()> {
    let input = input.as_ref();
    let output = output.as_ref();

    let text = convert(
        &fs::read_to_string(input).with_context(|| format!("Reading {}", input.display()))?,
    )?;
    crate::output::write_new(&[(output, text.as_bytes())])
}

pub fn convert(source: &str) -> Result<String> {
    let timeline = parse(source)?;
    timeline.render()
}

fn one_decimal(value: f64) -> bool {
    value.is_finite() && value >= 0.0 && (value * 10.0).fract() == 0.0
}

fn generated_schema() -> Result<Value> {
    // Describe log-specific field constraints with typed keywords, e.g. Map permits regionName.
    #[derive(Serialize)]
    struct Condition {
        r#if: Properties<ConstLog>,
        then: Properties<FieldNames>,
    }
    #[derive(Serialize)]
    struct Properties<T> {
        properties: BTreeMap<&'static str, T>,
    }
    #[derive(Serialize)]
    struct ConstLog {
        r#const: LogType,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct FieldNames {
        property_names: EnumConstraint,
    }
    #[derive(Serialize)]
    struct EnumConstraint {
        r#enum: &'static [&'static str],
    }
    let mut schema = serde_json::to_value(schema_for!(Timeline))?;
    // An absent optional field is valid; an explicit null would silently become None.
    fn forbid_null(value: &mut Value) {
        match value {
            Value::Object(object) => {
                if let Some(Value::Array(types)) = object.get_mut("type") {
                    types.retain(|kind| kind != "null");
                }
                if let Some(Value::Array(variants)) = object.get_mut("anyOf") {
                    variants.retain(|variant| {
                        variant.get("type") != Some(&Value::String("null".into()))
                    });
                }
                for child in object.values_mut() {
                    forbid_null(child);
                }
            }
            Value::Array(values) => {
                for child in values {
                    forbid_null(child);
                }
            }
            _ => {}
        }
    }
    forbid_null(&mut schema);
    // Use the same Rust log definitions for editor schemas and conversion validation.
    let logs = schema
        .pointer("/$defs/LogType/enum")
        .context("Missing network log enum in generated schema")?
        .as_array()
        .context("Missing network log enum in generated schema")?
        .clone();
    let conditions: Result<Vec<Value>> = logs
        .into_iter()
        .map(|name| {
            let log: LogType = serde_json::from_value(name)?;
            let fields = validation::known_fields(&log);
            Ok(serde_json::to_value(Condition {
                r#if: Properties {
                    properties: BTreeMap::from([("log", ConstLog { r#const: log })]),
                },
                then: Properties {
                    properties: BTreeMap::from([(
                        "fields",
                        FieldNames {
                            property_names: EnumConstraint { r#enum: fields },
                        },
                    )]),
                },
            })?)
        })
        .collect();
    schema
        .pointer_mut("/$defs/NetworkSync")
        .and_then(Value::as_object_mut)
        .context("Missing network sync in generated schema")?
        .insert("allOf".into(), Value::Array(conditions?));
    Ok(schema)
}

fn garde_time(value: &f64, _: &()) -> garde::Result {
    garde_check(
        one_decimal(*value),
        "Time must be nonnegative with at most one decimal place",
    )
}
fn garde_duration(value: &f64, _: &()) -> garde::Result {
    garde_check(value.is_finite() && *value > 0.0, "Invalid duration")
}
fn garde_check(valid: bool, message: &str) -> garde::Result {
    if valid {
        Ok(())
    } else {
        Err(garde::Error::new(message))
    }
}

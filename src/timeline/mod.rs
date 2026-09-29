use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use garde::Validate;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod render;
mod validation;

#[cfg(test)]
mod tests;

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Timeline {
    #[schemars(extend("const" = 1))]
    schema_version: u8,
    #[serde(default)]
    #[garde(inner(pattern(r#"^[^"\r\n]+$"#)))]
    #[schemars(extend("uniqueItems" = true))]
    hide_names: Vec<String>,
    #[garde(dive)]
    entries: Vec<Entry>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Entry {
    Event {
        #[garde(range(min = 0.0), custom(garde_time))]
        #[schemars(extend("multipleOf" = 0.1))]
        at: f64,
        #[garde(pattern(r#"^[^"\r\n]+$"#))]
        name: String,
        #[garde(inner(custom(garde_duration)))]
        #[schemars(extend("exclusiveMinimum" = 0))]
        duration: Option<f64>,
        sync: Option<Sync>,
        jump: Option<Jump>,
        #[garde(inner(pattern(r"^[^\r]+$")))]
        #[schemars(pattern(r"^[^\r]+$"))]
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
        phase: Option<String>,
    },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum Sync {
    Network(NetworkSync),
    Regex(RegexSync),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
enum LogType {
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
struct NetworkSync {
    log: LogType,
    #[schemars(extend("minProperties" = 1))]
    fields: BTreeMap<String, FieldPattern>,
    #[serde(default = "yes")]
    enabled: bool,
    #[schemars(extend("items" = {"type": "number", "minimum": 0, "multipleOf": 0.1}))]
    window: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegexSync {
    #[schemars(pattern(r#"^[^#"\r\n/]*$"#))]
    regex: String,
    #[serde(default = "yes")]
    enabled: bool,
    #[schemars(extend("items" = {"type": "number", "minimum": 0, "multipleOf": 0.1}))]
    window: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum FieldPattern {
    One(#[schemars(pattern(r#"^[^#"\r\n]*$"#))] String),
    Many(#[schemars(length(min = 1), inner(pattern(r#"^[^#"\r\n]*$"#)))] Vec<String>),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Jump {
    to: Destination,
    when: JumpWhen,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
enum Destination {
    Label(String),
    Time(#[schemars(range(min = 0.0), extend("multipleOf" = 0.1))] f64),
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum JumpWhen {
    Sync,
    Always,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhaseStart {
    #[garde(pattern(r"^[0-9A-F]+$"))]
    ability_id: String,
    #[garde(range(min = 0.0), custom(garde_time))]
    #[schemars(extend("multipleOf" = 0.1))]
    at: f64,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema, Validate)]
#[garde(allow_unvalidated)]
#[serde(deny_unknown_fields)]
struct Ability {
    #[garde(pattern(r"^[0-9A-F]+$"))]
    id: String,
    #[garde(pattern(r"^[^\r\n]+$"))]
    name: String,
    #[garde(inner(pattern(r"^[^\r\n]+$")))]
    #[schemars(pattern(r"^[^\r\n]+$"))]
    note: Option<String>,
    #[serde(default)]
    ignored: bool,
}

fn yes() -> bool {
    true
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
    let schema = generated_schema()?;
    jsonschema::validate(&schema, &value).map_err(|e| anyhow::anyhow!("JSON Schema: {e}"))?;
    let timeline: Timeline = serde_json::from_value(value)?;
    timeline.validate().context("Semantic field validation")?;
    timeline.validate_relations()?;
    Ok(timeline)
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
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .with_context(|| {
            format!(
                "Creating {} (existing files are preserved)",
                output.display()
            )
        })?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

pub fn convert(source: &str) -> Result<String> {
    let timeline = parse(source)?;
    timeline.render()
}

fn one_decimal(value: f64) -> bool {
    value.is_finite() && value >= 0.0 && (value * 10.0).fract() == 0.0
}

fn generated_schema() -> Result<Value> {
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
    let logs = schema["$defs"]["LogType"]["enum"]
        .as_array()
        .context("Missing network log enum in generated schema")?
        .clone();
    let conditions: Result<Vec<Value>> = logs
        .into_iter()
        .map(|name| {
            let log: LogType = serde_json::from_value(name.clone())?;
            Ok(serde_json::json!({
                "if": {"properties": {"log": {"const": name}}},
                "then": {"properties": {"fields": {"propertyNames": {"enum": validation::known_fields(&log)}}}}
            }))
        })
        .collect();
    schema["$defs"]["NetworkSync"]["allOf"] = Value::Array(conditions?);
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

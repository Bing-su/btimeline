use anyhow::{Context, Result, ensure};

// This covers encounter clocks and source timestamps through year 2286 while retaining millisecond precision.
const MAX_MILLISECONDS: i64 = 10_000_000_000_000;

fn seconds_to_ms(value: f64) -> Result<i64> {
    ensure!(value.is_finite(), "Seconds must be finite");
    let milliseconds = value * 1000.0;
    ensure!(
        milliseconds.abs() <= MAX_MILLISECONDS as f64,
        "Seconds out of range"
    );
    let rounded = milliseconds.round();
    // Allow binary representation noise, not a real fraction of a millisecond.
    let tolerance = (milliseconds.abs() * f64::EPSILON * 2.0).max(0.000_000_1);
    ensure!(
        (milliseconds - rounded).abs() <= tolerance,
        "Seconds must have millisecond precision"
    );
    Ok(rounded as i64)
}

fn transform_number(value: &mut serde_json::Value, to_seconds: bool) -> Result<()> {
    match value {
        serde_json::Value::Null => Ok(()),
        serde_json::Value::Array(items) => {
            for item in items {
                transform_number(item, to_seconds)?;
            }
            Ok(())
        }
        serde_json::Value::Object(values) => {
            for value in values.values_mut() {
                transform_number(value, to_seconds)?;
            }
            Ok(())
        }
        serde_json::Value::Number(number) => {
            if to_seconds {
                let ms = number
                    .as_i64()
                    .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok()))
                    .context("Invalid millisecond time")?;
                ensure!(
                    ms.unsigned_abs() <= MAX_MILLISECONDS.unsigned_abs(),
                    "Milliseconds out of range"
                );
                *number = if ms % 1000 == 0 {
                    serde_json::Number::from(ms / 1000)
                } else {
                    serde_json::Number::from_f64(ms as f64 / 1000.0).context("Invalid seconds")?
                };
            } else {
                let seconds = number.as_f64().context("Invalid seconds")?;
                *number = serde_json::Number::from(seconds_to_ms(seconds)?);
            }
            Ok(())
        }
        _ => anyhow::bail!("Expected a numeric time"),
    }
}

// Rename field selectors with their exported values, e.g. cast_ms -> cast_seconds.
fn field_times(fields: &mut [serde_json::Value]) {
    for field in fields {
        if let serde_json::Value::String(name) = field
            && let Some(base) = name.strip_suffix("_ms")
        {
            *name = format!("{base}_seconds");
        }
    }
}

// Convert one object together so nested time fields keep duplicate-name checks.
fn object_times(
    object: &mut serde_json::Map<String, serde_json::Value>,
    to_seconds: bool,
) -> Result<()> {
    let keys: Vec<String> = object.keys().cloned().collect();
    for key in keys {
        let count_name = match (to_seconds, key.as_str()) {
            (true, "over2500Ms") => Some("over2_5Seconds"),
            (true, "over10000Ms") => Some("over10Seconds"),
            (false, "over2_5Seconds") => Some("over2500Ms"),
            (false, "over10Seconds") => Some("over10000Ms"),
            _ => None,
        };
        if let Some(name) = count_name {
            let count = object.remove(&key).context("Missing count")?;
            ensure!(!object.contains_key(name), "Duplicate count field");
            object.insert(name.into(), count);
            continue;
        }
        if !to_seconds && (key.ends_with("_ms") || key.ends_with("Ms")) {
            anyhow::bail!("Millisecond field is not accepted: {key}");
        }
        if key == "removeProperty"
            && let Some(serde_json::Value::String(property)) = object.get_mut(&key)
        {
            if to_seconds && property == "cast_ms" {
                *property = "cast_seconds".into();
            }
            if !to_seconds && property == "cast_ms" {
                anyhow::bail!("Millisecond property is not accepted");
            }
            if !to_seconds && property == "cast_seconds" {
                *property = "cast_ms".into();
            }
        }
        if key == "fields"
            && to_seconds
            && let Some(serde_json::Value::Array(fields)) = object.get_mut(&key)
        {
            field_times(fields);
        }
        let replacement = if to_seconds {
            key.strip_suffix("_ms")
                .map(|base| format!("{base}_seconds"))
                .or_else(|| key.strip_suffix("Ms").map(|base| format!("{base}Seconds")))
        } else {
            key.strip_suffix("_seconds")
                .map(|base| format!("{base}_ms"))
                .or_else(|| key.strip_suffix("Seconds").map(|base| format!("{base}Ms")))
        };
        if let Some(replacement) = replacement {
            let mut value = object.remove(&key).context("Missing time field")?;
            transform_number(&mut value, to_seconds)
                .with_context(|| format!("Invalid time field: {key}"))?;
            ensure!(
                !object.contains_key(&replacement),
                "Duplicate time field: {replacement}"
            );
            object.insert(replacement, value);
        } else if let Some(child) = object.get_mut(&key) {
            json_times(child, to_seconds)?;
        }
    }

    Ok(())
}

pub(crate) fn json_times(value: &mut serde_json::Value, to_seconds: bool) -> Result<()> {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                json_times(item, to_seconds)?;
            }
        }
        serde_json::Value::Object(object) => {
            object_times(object, to_seconds)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::json_times;

    #[test]
    fn export_preserves_decimal_seconds() -> anyhow::Result<()> {
        let mut value = serde_json::json!({"items":[{"at_ms":16341,"cast_ms":500}],"scope":{"start_ms":0,"end_ms":4000}});
        json_times(&mut value, true)?;
        anyhow::ensure!(
            value.to_string()
                == r#"{"items":[{"at_seconds":16.341,"cast_seconds":0.5}],"scope":{"end_seconds":4,"start_seconds":0}}"#
        );
        Ok(())
    }
    #[test]
    fn json_times_round_trip_signed_arrays_and_counts() -> anyhow::Result<()> {
        let original = serde_json::json!({"at_ms":-1,"durationMs":1234,"offsetRangeMs":[-1500,0,250],"over2500Ms":3,"over10000Ms":2});
        let mut converted = original.clone();
        json_times(&mut converted, true)?;
        anyhow::ensure!(converted["at_seconds"] == serde_json::json!(-0.001));
        anyhow::ensure!(converted["durationSeconds"] == serde_json::json!(1.234));
        anyhow::ensure!(converted["over2_5Seconds"] == 3 && converted["over10Seconds"] == 2);
        json_times(&mut converted, false)?;
        anyhow::ensure!(converted == original);
        let mut timestamp = serde_json::json!({"sourceAtMs": 1_800_000_000_001_i64});
        let expected = timestamp.clone();
        json_times(&mut timestamp, true)?;
        json_times(&mut timestamp, false)?;
        anyhow::ensure!(timestamp == expected);
        Ok(())
    }
}

use std::fmt;
use std::io::Write;
use std::sync::LazyLock;

use anyhow::{Result, anyhow, bail};
use camino::Utf8PathBuf;
use serde_json::{Map, Value};
use tracing::info;
use usage::{Args, Run, ValueEnum};

#[derive(Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Ndjson,
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Ndjson => write!(f, "ndjson"),
        }
    }
}

#[derive(Debug, Args)]
pub struct FFLogsCommand {
    /// FFLogs report code
    #[usage(arg)]
    report_code: String,

    /// Fight ID within the report
    #[usage(arg)]
    fight_id: i64,

    /// FFLogs API key
    #[usage(short = 'k', long, env = "FFLOGS_API_KEY")]
    api_key: String,

    /// directory for output files, default is current directory
    /// file will be named {report_code}_{fight_id}.{format}
    #[usage(short = 'o', long)]
    output: Option<Utf8PathBuf>,

    /// Output file format
    #[usage(short = 'f', long, value_enum, default = "ndjson")]
    format: OutputFormat,
}

const BASE_URL: &str = "https://www.fflogs.com/v1";
static CLIENT: LazyLock<ureq::Agent> = LazyLock::new(ureq::agent);

fn get_fight(report_code: &str, fight_id: i64, api_key: &str) -> Result<Map<String, Value>> {
    let url = format!("{}/report/fights/{}", BASE_URL, report_code);
    let resp = CLIENT
        .get(&url)
        .query("api_key", api_key)
        .query("translate", "false")
        .call()?;
    info!(
        "Fetched fight data for report_code: {}, fight_id: {}",
        report_code, fight_id
    );
    let data: Value = resp.into_body().read_json()?;
    let fights = data
        .get("fights")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Invalid fight data format"))?;
    fights
        .iter()
        .filter_map(Value::as_object)
        .find(|fight| fight.get("id").and_then(Value::as_i64) == Some(fight_id))
        .cloned()
        .ok_or_else(|| anyhow!("Fight with ID {} not found", fight_id))
}

fn next_page_start(data: &Value, current: i64) -> Result<Option<i64>> {
    data.get("nextPageTimestamp")
        .map(|value| {
            let next = value
                .as_i64()
                .ok_or_else(|| anyhow!("Invalid nextPageTimestamp format"))?;
            if next <= current {
                bail!("nextPageTimestamp did not advance");
            }
            Ok(next)
        })
        .transpose()
}

fn get_all_fight_events(
    report_code: &str,
    fight_id: i64,
    api_key: &str,
) -> Result<Vec<Map<String, Value>>> {
    let fight = get_fight(report_code, fight_id, api_key)?;
    let start = fight
        .get("start_time")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("Failed to get start time"))?;
    let end = fight
        .get("end_time")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("Failed to get end time"))?;

    let mut events = Vec::new();

    let mut page_start = start;
    while page_start < end {
        let url = format!("{}/report/events/summary/{}", BASE_URL, report_code);
        let resp = CLIENT
            .get(&url)
            .query("api_key", api_key)
            .query("translate", "false")
            .query("start", page_start.to_string())
            .query("end", end.to_string())
            .query("hostility", "1")
            .call()?;

        info!(
            "Fetched fight events for report_code: {}, fight_id: {}, from start: {} to end: {}",
            report_code, fight_id, page_start, end
        );
        let data: Value = resp.into_body().read_json()?;
        let new_events = data
            .get("events")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Failed to get events"))?;
        events.extend(new_events.iter().filter_map(Value::as_object).cloned());

        let Some(next) = next_page_start(&data, page_start)? else {
            break;
        };
        page_start = next;
    }

    events.sort_by_key(|event| {
        event
            .get("timestamp")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    });
    Ok(events)
}

fn save_events_to_file(
    events: &[Map<String, Value>],
    output: &Option<impl AsRef<std::path::Path>>,
    filename: &str,
    format: OutputFormat,
) -> Result<()> {
    let output_dir = match &output {
        Some(path) => path.as_ref().to_path_buf(),
        None => std::env::current_dir()?,
    };

    std::fs::create_dir_all(&output_dir)?;

    let filename = output_dir.join(filename);
    let mut file = std::fs::File::create(&filename)?;
    match format {
        OutputFormat::Json => writeln!(file, "{}", serde_json::to_string(&events)?)?,
        OutputFormat::Ndjson => {
            for event in events {
                writeln!(file, "{}", serde_json::to_string(&event)?)?;
            }
        }
    }

    Ok(())
}

impl Run for FFLogsCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let events = get_all_fight_events(&self.report_code, self.fight_id, &self.api_key)?;
        let filename = format!("{}_{}.{}", self.report_code, self.fight_id, self.format);
        save_events_to_file(&events, &self.output, &filename, self.format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn page_cursor_must_advance() {
        assert_eq!(next_page_start(&json!({}), 10).unwrap(), None);
        assert_eq!(
            next_page_start(&json!({ "nextPageTimestamp": 11 }), 10).unwrap(),
            Some(11)
        );
        assert!(next_page_start(&json!({ "nextPageTimestamp": 10 }), 10).is_err());
        assert!(next_page_start(&json!({ "nextPageTimestamp": "11" }), 10).is_err());
    }
}

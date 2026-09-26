use std::{fs, path::Path, time::Duration};

use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::info;
use usage::{Args, Run, ValueEnum};

#[derive(ValueEnum)]
enum OutputFormat {
    Json,
    JsonPretty,
}

// Avoid deriving Debug so credentials cannot appear in debug output.
#[derive(Args)]
pub struct FFLogsCommand {
    /// FFLogs report code
    #[usage(arg)]
    report_code: String,
    /// Fight ID within the report
    #[usage(arg)]
    fight_id: i64,
    /// FFLogs client ID
    #[usage(short = 'i', long, env = "FFLOGS_CLIENT_ID")]
    client_id: String,
    /// FFLogs client secret
    #[usage(short = 's', long, env = "FFLOGS_CLIENT_SECRET")]
    client_secret: String,
    /// Output directory; saves {report_code}_{fight_id}.json
    #[usage(short = 'o', long)]
    output: Option<Utf8PathBuf>,

    /// Output format
    #[usage(short = 'f', long, value_enum, default = "json")]
    format: OutputFormat,
}

const TOKEN_URL: &str = "https://www.fflogs.com/oauth/token";
const API_URL: &str = "https://www.fflogs.com/api/v2/client";
const METADATA: &str = r#"
query TimelineMetadata($code: String!, $fightIDs: [Int]) {
  reportData { report(code: $code, allowUnlisted: true) {
    code title startTime endTime revision visibility
    archiveStatus { isArchived isAccessible archiveDate }
    fights(fightIDs: $fightIDs, translate: true) {
      id name encounterID originalEncounterID difficulty kill inProgress
      startTime endTime combatTime gameZone { id name }
      enemyNPCs { id gameID instanceCount groupCount petOwner }
      enemyPets { id gameID instanceCount groupCount petOwner }
      enemyPlayers
      friendlyNPCs { id gameID instanceCount groupCount petOwner }
      friendlyPets { id gameID instanceCount groupCount petOwner }
      friendlyPlayers lastPhase lastPhaseAsAbsoluteIndex lastPhaseIsIntermission
      phaseTransitions { id startTime }
    }
    phases { encounterID separatesWipes phases { id name isIntermission } }
    masterData(translate: true) {
      lang logVersion gameVersion
      actors { id gameID name type subType petOwner }
      abilities { gameID name icon type }
    }
  } }
}"#;
const EVENTS: &str = r#"
query TimelineEvents($code: String!, $fightIDs: [Int]!, $start: Float!, $end: Float!) {
  reportData { report(code: $code, allowUnlisted: true) {
    events(fightIDs: $fightIDs, startTime: $start, endTime: $end,
      dataType: All, limit: 10000, useAbilityIDs: true, useActorIDs: true,
      includeResources: true, translate: true) { data nextPageTimestamp }
  } }
}"#;

#[derive(Deserialize)]
struct Token {
    access_token: String,
    token_type: String,
}

struct Client {
    agent: ureq::Agent,
    token: String,
    api_url: String,
}

impl Client {
    fn authenticate(token_url: &str, api_url: &str, id: &str, secret: &str) -> Result<Self> {
        ensure!(
            !id.trim().is_empty() && !secret.trim().is_empty(),
            "Empty FFLogs credentials"
        );
        // Disable redirects so credentials cannot be forwarded to another endpoint.
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .build()
            .new_agent();
        let basic = STANDARD.encode(format!("{id}:{secret}"));
        let mut response = agent
            .post(token_url)
            .header("Authorization", format!("Basic {basic}"))
            .send_form([("grant_type", "client_credentials")])
            .map_err(|_| anyhow!("FFLogs token request failed"))?;
        ensure!(
            response.status().is_success(),
            "FFLogs token HTTP {}",
            response.status()
        );
        let token: Token = response
            .body_mut()
            .read_json()
            .map_err(|_| anyhow!("Invalid FFLogs token response"))?;
        ensure!(
            !token.access_token.trim().is_empty(),
            "Empty FFLogs access token"
        );
        ensure!(
            token.token_type.eq_ignore_ascii_case("Bearer"),
            "Unsupported FFLogs token type"
        );
        Ok(Self {
            agent,
            token: token.access_token,
            api_url: api_url.into(),
        })
    }

    fn query(&self, query: &str, variables: Value) -> Result<Value> {
        let mut response = self
            .agent
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send_json(json!({"query": query, "variables": variables}))
            .map_err(|_| anyhow!("FFLogs GraphQL HTTP request failed"))?;
        ensure!(
            response.status().is_success(),
            "FFLogs GraphQL HTTP {}",
            response.status()
        );

        let data: Value = response
            .body_mut()
            .read_json()
            .context("Invalid FFLogs GraphQL JSON")?;
        if let Some(errors) = data.get("errors") {
            ensure!(
                errors.as_array().is_some_and(Vec::is_empty),
                "FFLogs GraphQL errors returned"
            );
        }
        data.pointer("/data/reportData/report")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| anyhow!("Report missing or inaccessible"))
    }

    fn collect(&self, code: &str, fight_id: i64) -> Result<Value> {
        ensure!(
            !code.is_empty() && code.bytes().all(|b| b.is_ascii_alphanumeric()),
            "Invalid report code"
        );
        ensure!(
            (1..=i32::MAX as i64).contains(&fight_id),
            "Invalid fight ID"
        );
        let mut report = self.query(METADATA, json!({"code": code, "fightIDs": [fight_id]}))?;
        ensure!(
            report
                .pointer("/archiveStatus/isAccessible")
                .and_then(Value::as_bool)
                == Some(true),
            "Report archive is inaccessible or archive status is invalid"
        );
        let fights = report
            .get("fights")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Invalid fights array"))?;
        let fight = fights
            .iter()
            .find(|v| v.get("id").and_then(Value::as_i64) == Some(fight_id))
            .cloned()
            .ok_or_else(|| anyhow!("Fight {fight_id} not found"))?;
        ensure!(
            fight.get("inProgress").and_then(Value::as_bool) == Some(false),
            "Fight is still uploading or has invalid status"
        );
        let start = fight
            .get("startTime")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Invalid fight startTime"))?;
        let end = fight
            .get("endTime")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Invalid fight endTime"))?;
        ensure!(
            start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
            "Invalid fight time range"
        );
        report["fights"] = json!([fight]);
        let mut cursor = start;
        let mut events = Vec::new();
        loop {
            let page = self.query(
                EVENTS,
                json!({"code": code, "fightIDs": [fight_id], "start": cursor, "end": end}),
            )?;
            let page = page
                .get("events")
                .ok_or_else(|| anyhow!("Missing event page"))?;
            let rows = page
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Invalid event array"))?;
            ensure!(rows.iter().all(Value::is_object), "Invalid event record");
            // Preserve API order, duplicates and optional fields for later timeline analysis.
            events.extend(rows.iter().cloned());
            let next = page
                .get("nextPageTimestamp")
                .ok_or_else(|| anyhow!("Missing pagination cursor"))?;
            if next.is_null() {
                break;
            }
            let next = next
                .as_f64()
                .ok_or_else(|| anyhow!("Invalid pagination cursor"))?;
            ensure!(
                next.is_finite() && next > cursor && next <= end,
                "Pagination cursor did not advance within fight range"
            );
            cursor = next;
        }
        info!(
            report = code,
            fight = fight_id,
            events = events.len(),
            "Collected FFLogs fight"
        );
        Ok(json!({"report": report, "events": events}))
    }
}

fn save(
    data: &Value,
    directory: impl AsRef<Path>,
    filename: &str,
    format: OutputFormat,
) -> Result<()> {
    let dir = directory.as_ref();
    fs::create_dir_all(dir)?;
    let bytes = match format {
        OutputFormat::Json => serde_json::to_vec(data)?,
        OutputFormat::JsonPretty => serde_json::to_vec_pretty(data)?,
    };

    fs::write(dir.join(filename), bytes)?;
    Ok(())
}

impl Run for FFLogsCommand {
    type Output = Result<()>;
    fn run(self) -> Result<()> {
        let client =
            Client::authenticate(TOKEN_URL, API_URL, &self.client_id, &self.client_secret)?;
        let data = client.collect(&self.report_code, self.fight_id)?;
        save(
            &data,
            self.output.unwrap_or_default(),
            &format!("{}_{}.json", self.report_code, self.fight_id),
            self.format,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    // Real local HTTP exercises the same form, headers and GraphQL requests as production.
    fn server(responses: Vec<(u16, Value)>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body_bytes = vec![0; length];
                stream.read_exact(&mut body_bytes).unwrap();
                requests.push(format!(
                    "{headers}{}",
                    String::from_utf8(body_bytes).unwrap()
                ));
                let body = body.to_string();
                write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (url, handle)
    }
    fn token() -> (u16, Value) {
        (
            200,
            json!({"access_token":"test-token", "token_type":"Bearer"}),
        )
    }
    fn report(value: Value) -> (u16, Value) {
        (200, json!({"data":{"reportData":{"report":value}}}))
    }
    fn metadata() -> (u16, Value) {
        report(json!({"archiveStatus":{"isAccessible":true},"phases":null,
            "masterData":{"lang":"en","actors":[],"abilities":[]},
            "fights":[{"id":1,"startTime":10,"endTime":30,"inProgress":false,"phaseTransitions":null}]}))
    }
    #[test]
    fn collects_two_pages_and_saves_raw_json() {
        let (url, handle) = server(vec![
            token(),
            metadata(),
            report(
                json!({"events":{"data":[{"timestamp":11,"optional":{"x":1}}],"nextPageTimestamp":20}}),
            ),
            report(json!({"events":{"data":[{"timestamp":21}],"nextPageTimestamp":null}})),
        ]);
        let client = Client::authenticate(&url, &url, "id", "secret").unwrap();
        let data = client.collect("example", 1).unwrap();
        assert_eq!(data["events"].as_array().unwrap().len(), 2);
        assert_eq!(data["events"][0]["optional"]["x"], 1);
        assert!(data["report"]["phases"].is_null());
        let dir = std::env::temp_dir().join(format!("btimeline-success-{}", std::process::id()));
        save(&data, &dir, "example_1.json", OutputFormat::Json).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(dir.join("example_1.json")).unwrap())
                .unwrap(),
            data
        );
        fs::remove_dir_all(dir).unwrap();
        let requests = handle.join().unwrap();
        assert!(requests[0].contains("Basic aWQ6c2VjcmV0"));
        assert!(requests[0].contains("grant_type=client_credentials"));
        assert!(requests[1].contains("Bearer test-token"));
        assert!(requests[2].contains("includeResources: true"));
        let body: Value =
            serde_json::from_str(requests[3].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["variables"]["start"], 20.0);
    }
    #[test]
    fn rejects_bad_authentication() {
        for response in [
            (401, json!({})),
            (200, json!({"access_token":"", "token_type":"Bearer"})),
            (200, json!({"access_token":"test", "token_type":"Other"})),
        ] {
            let (url, handle) = server(vec![response]);
            assert!(Client::authenticate(&url, &url, "id", "secret").is_err());
            handle.join().unwrap();
        }
    }
    #[test]
    fn rejects_missing_or_unavailable_fights() {
        let mut unavailable = metadata().1;
        unavailable["data"]["reportData"]["report"]["archiveStatus"]["isAccessible"] = json!(false);
        let mut uploading = metadata().1;
        uploading["data"]["reportData"]["report"]["fights"][0]["inProgress"] = json!(true);
        let mut missing_fight = metadata().1;
        missing_fight["data"]["reportData"]["report"]["fights"] = json!([]);
        for response in [
            report(Value::Null),
            (200, unavailable),
            (200, uploading),
            (200, missing_fight),
        ] {
            let (url, handle) = server(vec![token(), response]);
            let client = Client::authenticate(&url, &url, "id", "secret").unwrap();
            assert!(client.collect("example", 1).is_err());
            handle.join().unwrap();
        }
    }
    #[test]
    fn failures_preserve_previous_output() {
        let dir = std::env::temp_dir().join(format!("btimeline-failure-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("example_1.json");
        fs::write(&target, "previous").unwrap();
        for page in [
            report(json!({"events":{"data":[],"nextPageTimestamp":10}})),
            report(json!({"events":{"data":[],"nextPageTimestamp":9}})),
            (200, json!({"errors":[{"message":"denied"}]})),
            report(json!({"events":{"data":{},"nextPageTimestamp":null}})),
            (503, json!({})),
        ] {
            let (url, handle) = server(vec![token(), metadata(), page]);
            let client = Client::authenticate(&url, &url, "id", "secret").unwrap();
            let result = client
                .collect("example", 1)
                .and_then(|data| save(&data, &dir, "example_1.json", OutputFormat::Json));
            assert!(result.is_err());
            assert_eq!(fs::read_to_string(&target).unwrap(), "previous");
            handle.join().unwrap();
        }
        // Writing to a directory must fail without creating extra files.
        fs::create_dir(dir.join("blocked.json")).unwrap();
        assert!(save(&json!({}), &dir, "blocked.json", OutputFormat::Json).is_err());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}

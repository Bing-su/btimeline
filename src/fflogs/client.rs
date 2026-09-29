use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, ensure};
use backon::{BlockingRetryable, ExponentialBuilder};
use base64::{Engine, engine::general_purpose::STANDARD};
use garde::Validate;
use serde::Deserialize;
use serde_json::{Value, json};

use super::model::CollectedLog;
use tracing::info;

pub(super) const TOKEN_URL: &str = "https://www.fflogs.com/oauth/token";
pub(super) const API_URL: &str = "https://www.fflogs.com/api/v2/client";
const METADATA: &str = r#"
query TimelineMetadata($code: String!, $fightIDs: [Int]) {
  reportData { report(code: $code, allowUnlisted: true) {
    code title startTime endTime revision visibility
    archiveStatus { isArchived isAccessible archiveDate }
    fights(fightIDs: $fightIDs, translate: true) {
      id name encounterID originalEncounterID difficulty kill inProgress
      startTime endTime combatTime fightPercentage bossPercentage gameZone { id name }
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

pub(super) struct Client {
    agent: ureq::Agent,
    token: String,
    api_url: String,
}

impl Client {
    pub(super) fn authenticate(
        token_url: &str,
        api_url: &str,
        id: &str,
        secret: &str,
    ) -> Result<Self> {
        ensure!(
            !id.trim().is_empty() && !secret.trim().is_empty(),
            "Empty FFLogs credentials"
        );
        // Disable redirects so credentials cannot be forwarded to another endpoint.
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .new_agent();
        let basic = STANDARD.encode(format!("{id}:{secret}"));
        let mut response = agent
            .post(token_url)
            .header("Authorization", format!("Basic {basic}"))
            .send_form([("grant_type", "client_credentials")])
            .context("FFLogs token request failed")?;
        ensure!(
            response.status().is_success(),
            "FFLogs token HTTP {}",
            response.status()
        );
        let token: Token = response
            .body_mut()
            .read_json()
            .context("Invalid FFLogs token response")?;
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
        let body = json!({"query": query, "variables": variables});
        let retry_after = std::rc::Rc::new(std::cell::Cell::new(None));
        let sleep_delay = retry_after.clone();
        // Queries are read-only: backon retries transient failures twice, never auth or GraphQL errors.
        let mut response = (|| {
            retry_after.set(None);
            let response = self
                .agent
                .post(&self.api_url)
                .header("Authorization", format!("Bearer {}", self.token))
                .send_json(&body)
                .map_err(|error| {
                    let transient = matches!(
                        error,
                        ureq::Error::Io(_)
                            | ureq::Error::Timeout(_)
                            | ureq::Error::ConnectionFailed
                    );
                    (anyhow!("FFLogs GraphQL HTTP request failed"), transient)
                })?;
            if matches!(response.status().as_u16(), 429 | 502 | 503 | 504) {
                let delay = response
                    .headers()
                    .get("Retry-After")
                    .map(|value| {
                        let seconds = value
                            .to_str()
                            .ok()
                            .and_then(|s| s.parse::<u64>().ok())
                            .context("Retry-After is not a delay in seconds; retry later")?;
                        ensure!(seconds <= 60, "Retry-After exceeds 60 seconds; retry later");
                        Ok(Duration::from_secs(seconds))
                    })
                    .transpose()
                    .map_err(|error: anyhow::Error| (error, false))?;
                retry_after.set(delay);
                return Err((anyhow!("FFLogs GraphQL HTTP {}", response.status()), true));
            }
            Ok(response)
        })
        .retry(
            ExponentialBuilder::default()
                .with_min_delay(Duration::from_secs(1))
                .with_factor(2.0)
                .with_max_times(2),
        )
        .when(|(_, transient)| *transient)
        .sleep(move |backoff| {
            // BlockingRetry has no delay override; honor server delays, e.g. Retry-After: 0.
            let delay = sleep_delay.take().unwrap_or(backoff);
            std::thread::sleep(if cfg!(test) {
                Duration::from_millis(1)
            } else {
                delay
            });
        })
        .call()
        .map_err(|(error, _)| error)?;
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
            .context("Report missing or inaccessible")
    }

    pub(super) fn metadata(&self, code: &str, fight_id: Option<i64>) -> Result<Value> {
        validate_selection(code, fight_id)?;
        let report = self.query(
            METADATA,
            json!({"code": code, "fightIDs": fight_id.map(|id| vec![id])}),
        )?;
        ensure!(
            report
                .pointer("/archiveStatus/isAccessible")
                .and_then(Value::as_bool)
                == Some(true),
            "Report archive is inaccessible or archive status is invalid"
        );
        ensure!(
            report.get("fights").is_some_and(Value::is_array),
            "Invalid fights array"
        );
        Ok(report)
    }

    #[cfg(test)]
    pub(super) fn collect(&self, code: &str, fight_id: i64) -> Result<Value> {
        let report = self.metadata(code, Some(fight_id))?;
        Ok(serde_json::to_value(self.collect_from_report(
            code,
            fight_id,
            report,
            Some(fight_id),
        )?)?)
    }

    pub(super) fn collect_from_report(
        &self,
        code: &str,
        fight_id: i64,
        mut report: Value,
        metadata_fight_id: Option<i64>,
    ) -> Result<CollectedLog> {
        validate_selection(code, Some(fight_id))?;
        let fights = report
            .get("fights")
            .and_then(Value::as_array)
            .context("Invalid fights array")?;
        let fight = fights
            .iter()
            .find(|v| v.get("id").and_then(Value::as_i64) == Some(fight_id))
            .cloned()
            .with_context(|| format!("Fight {fight_id} not found"))?;
        ensure!(
            fight.get("inProgress").and_then(Value::as_bool) == Some(false),
            "Fight is still uploading or has invalid status"
        );
        let start = fight
            .get("startTime")
            .and_then(Value::as_f64)
            .context("Invalid fight startTime")?;
        let end = fight
            .get("endTime")
            .and_then(Value::as_f64)
            .context("Invalid fight endTime")?;
        ensure!(
            start.is_finite() && end.is_finite() && start >= 0.0 && end >= start,
            "Invalid fight time range"
        );
        *report.get_mut("fights").context("Invalid fights array")? = json!([fight]);
        let mut cursor = start;
        let mut events = Vec::new();
        let mut page_starts = Vec::new();
        loop {
            let page = self.query(
                EVENTS,
                json!({"code": code, "fightIDs": [fight_id], "start": cursor, "end": end}),
            )?;
            let page = page.get("events").context("Missing event page")?;
            let rows = page
                .get("data")
                .and_then(Value::as_array)
                .context("Invalid event array")?;
            ensure!(rows.iter().all(Value::is_object), "Invalid event record");
            page_starts.push(cursor);
            // Preserve API order, duplicates and optional fields for later timeline analysis.
            events.extend(rows.iter().cloned());
            let next = page
                .get("nextPageTimestamp")
                .context("Missing pagination cursor")?;
            if next.is_null() {
                break;
            }
            let next = next.as_f64().context("Invalid pagination cursor")?;
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
        // Keep provenance with raw data so one atomic save commits both, without credentials.
        let collected_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
        let data: CollectedLog = serde_json::from_value(json!({"report": report, "events": events, "collection": {
            "schemaVersion": 1,
            "toolVersion": env!("CARGO_PKG_VERSION"),
            "collectedAtUnixMs": u64::try_from(collected_at)?,
            "reportCode": code, "fightID": fight_id,
            "pageCount": page_starts.len(), "pageStartTimes": page_starts, "eventCount": events.len(),
            "complete": true, "nextPageTimestamp": null,
            "startTime": start, "endTime": end,
            "requests": {
                "metadata": {"query": METADATA, "variables": {"code": code, "fightIDs": metadata_fight_id.map(|id| vec![id])}},
                "events": {"query": EVENTS, "variables": {"code": code, "fightIDs": [fight_id], "start": start, "end": end}}
            }
        }})).context("Invalid collected log")?;
        data.validate().context("Invalid collected log")?;
        Ok(data)
    }
}

pub(super) fn validate_selection(code: &str, fight_id: Option<i64>) -> Result<()> {
    ensure!(
        !code.is_empty() && code.bytes().all(|b| b.is_ascii_alphanumeric()),
        "Invalid report code"
    );
    ensure!(
        fight_id.is_none_or(|id| (1..=i32::MAX as i64).contains(&id)),
        "Invalid fight ID"
    );
    Ok(())
}

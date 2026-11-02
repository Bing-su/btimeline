//! Separate candidate discovery from event downloads so plans can be reviewed and resumed.

use std::collections::BTreeSet;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};
use camino::{Utf8Path, Utf8PathBuf};
use garde::Validate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use usage_rs::{Args, Run};

use super::client::{API_URL, Client, TOKEN_URL, validate_selection};
use super::model::{CollectedLog, Fight};

mod selection;
use selection::{FightMetadata, Purpose, prioritize};

const CANDIDATES: &str = r#"
query TimelineCandidates($code: String!) {
  rateLimitData { limitPerHour pointsSpentThisHour pointsResetIn }
  reportData { report(code: $code, allowUnlisted: true) {
    code archiveStatus { isAccessible }
    fights(translate: true) {
      id name encounterID difficulty kill inProgress startTime endTime
      gameZone { id name } combatTime fightPercentage bossPercentage
      lastPhase lastPhaseAsAbsoluteIndex lastPhaseIsIntermission hasEcho
    }
  } }
}"#;

const ZONE_REPORTS: &str = r#"
query TimelineZoneReports($zone: Int!, $page: Int!) {
  rateLimitData { limitPerHour pointsSpentThisHour pointsResetIn }
  reportData { reports(gameZoneID: $zone, page: $page, limit: 100) {
    data { code } has_more_pages
  } }
}"#;

const RANKINGS: &str = r#"
query TimelineRankings($encounter: Int!, $difficulty: Int, $page: Int!, $partition: Int, $region: String) {
  rateLimitData { limitPerHour pointsSpentThisHour pointsResetIn }
  worldData { encounter(id: $encounter) {
    fightRankings(difficulty: $difficulty, page: $page, partition: $partition, serverRegion: $region)
  } }
}"#;

// Credentials deliberately have no Debug implementation, including when flattened into commands.
#[derive(Args)]
struct Access {
    #[usage(short = 'i', long, env = "FFLOGS_CLIENT_ID")]
    client_id: String,
    #[usage(short = 's', long, env = "FFLOGS_CLIENT_SECRET")]
    client_secret: String,
    /// Maximum GraphQL HTTP attempts, including quota probes and retries.
    #[usage(long, default = "100")]
    max_requests: usize,
    /// Stop before the next request once this many API points have been observed.
    #[usage(long, default = "1000")]
    max_points: f64,
}

impl Access {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty(),
            "Empty FFLogs credentials"
        );
        ensure!(
            self.max_requests > 0 && self.max_points.is_finite() && self.max_points > 0.0,
            "Invalid collection budget"
        );
        Ok(())
    }

    fn connect(&self) -> Result<Client> {
        self.validate()?;
        Client::authenticate(TOKEN_URL, API_URL, &self.client_id, &self.client_secret)?
            .with_budget(self.max_requests, self.max_points)
    }
}

#[derive(Args)]
pub struct FFLogsCollectCommand {
    /// Explicit report codes; omit to discover by game zone or encounter rankings.
    #[usage(arg)]
    reports: Vec<String>,
    #[usage(long)]
    encounter: Option<i64>,
    #[usage(long)]
    difficulty: Option<i64>,
    /// In-game zone ID; also enables discovery for unranked normal trials.
    #[usage(long)]
    game_zone: Option<i64>,
    #[usage(long)]
    partition: Option<i64>,
    #[usage(long)]
    region: Option<String>,
    /// First discovery page; later rankings pages broaden the sample beyond the fastest kills.
    #[usage(long, default = "1")]
    page: usize,
    #[usage(long, default = "1")]
    pages: usize,
    #[usage(long, default = "10")]
    max_reports: usize,
    /// Maximum selected fights after all inspected reports have been compared.
    #[usage(long, default = "100")]
    max_fights: usize,
    /// Candidate selection purpose: training favors long progress; verification includes short runs.
    #[usage(long, default = "training")]
    purpose: String,
    /// New JSON plan path, e.g. out/r12s.download-plan.json.
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
    #[usage(flatten)]
    access: Access,
}

#[derive(Args)]
pub struct FFLogsDownloadCommand {
    #[usage(arg)]
    plan: Utf8PathBuf,
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
    /// Maximum newly downloaded fights in this invocation; completed files do not count.
    #[usage(long, default = "100")]
    limit: usize,
    /// Maximum bytes published by this invocation; an individual request can be larger.
    #[usage(long, default = "2147483648")]
    max_bytes: u64,
    #[usage(flatten)]
    access: Access,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadPlan {
    schema_version: u32,
    #[serde(default)]
    purpose: Option<Purpose>,
    collected_at_unix_ms: u64,
    source: PlanSource,
    #[serde(default)]
    selection_policy: String,
    #[serde(default)]
    candidate_count: usize,
    fights: Vec<PlannedFight>,
    failures: Vec<Failure>,
    truncated: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSource {
    reports: Vec<String>,
    encounter: Option<i64>,
    difficulty: Option<i64>,
    game_zone: Option<i64>,
    partition: Option<i64>,
    region: Option<String>,
    first_page: usize,
    pages: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PlannedFight {
    report: String,
    fight: i64,
    name: String,
    encounter: i64,
    difficulty: i64,
    kill: bool,
    start_time: i64,
    end_time: i64,
    #[serde(flatten)]
    metadata: FightMetadata,
}

impl PlannedFight {
    fn validate(&self) -> Result<()> {
        validate_selection(&self.report, Some(self.fight))?;
        ensure!(
            self.encounter > 0
                && self.difficulty >= 0
                && !self.name.trim().is_empty()
                && self.start_time >= 0
                && self.end_time >= self.start_time,
            "Invalid planned fight"
        );
        self.metadata.validate(self.end_time - self.start_time)?;
        Ok(())
    }

    fn verify(&self, data: &CollectedLog) -> Result<()> {
        data.validate().context("Invalid downloaded collection")?;
        let fight = data
            .report
            .fights
            .first()
            .context("Missing downloaded fight")?;
        self.verify_fight(&data.report.code, fight)
    }

    fn verify_fight(&self, report: &str, fight: &Fight) -> Result<()> {
        ensure!(
            report == self.report
                && fight.id == self.fight
                && fight.encounter_id == self.encounter
                && fight.difficulty == self.difficulty
                && fight.start_time == self.start_time
                && fight.end_time == self.end_time
                && fight.kill == self.kill,
            "Downloaded fight differs from the plan"
        );
        let metadata: FightMetadata = serde_json::from_value(serde_json::to_value(fight)?)?;
        ensure!(
            metadata == self.metadata,
            "Downloaded fight metadata differs from the plan"
        );
        Ok(())
    }

    // Reject a stale plan before spending requests on event pages, e.g. a changed game zone.
    fn verify_metadata(&self, report: &Value) -> Result<()> {
        let code = report["code"].as_str().context("Missing report code")?;
        let [fight] = report["fights"]
            .as_array()
            .context("Missing report fights")?
            .as_slice()
        else {
            anyhow::bail!("Expected one downloaded fight")
        };
        self.verify_fight(code, &serde_json::from_value(fight.clone())?)
    }
}

#[derive(Deserialize, Serialize)]
struct Failure {
    report: String,
    fight: Option<i64>,
    error: String,
}

impl DownloadPlan {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 3,
            "Unsupported download plan version {}; regenerate with fflogs-collect",
            self.schema_version
        );
        let purpose = self.purpose.context("Missing collection purpose")?;
        ensure!(!self.fights.is_empty(), "Empty download plan");
        ensure!(
            self.selection_policy == purpose.policy() && self.candidate_count >= self.fights.len(),
            "Invalid download selection evidence"
        );
        let mut identities = BTreeSet::new();
        for fight in &self.fights {
            fight.validate()?;
            ensure!(
                self.source.encounter.is_none_or(|id| id == fight.encounter)
                    && self
                        .source
                        .difficulty
                        .is_none_or(|id| id == fight.difficulty)
                    && self
                        .source
                        .game_zone
                        .is_none_or(|id| Some(id) == fight.metadata.zone_id()),
                "Planned fight violates source filters"
            );
            ensure!(
                identities.insert((&fight.report, fight.fight)),
                "Duplicate planned fight"
            );
        }
        Ok(())
    }
}

fn now() -> Result<u64> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

fn write_json(path: &Utf8Path, data: &impl Serialize, replace: bool) -> Result<()> {
    if let Some(parent) = path.parent().filter(|path| !path.as_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(data)?;
    if replace {
        crate::output::write_replace(path.as_std_path(), |file| {
            use std::io::Write;
            Ok(file.write_all(&bytes)?)
        })
    } else {
        crate::output::write_new(&[(path.as_std_path(), &bytes)])
    }
}

fn candidate_fights(
    report: &Value,
    encounter: Option<i64>,
    difficulty: Option<i64>,
    game_zone: Option<i64>,
) -> Result<Vec<PlannedFight>> {
    let code = report["code"]
        .as_str()
        .context("Missing candidate report code")?;
    ensure!(
        report
            .pointer("/archiveStatus/isAccessible")
            .and_then(Value::as_bool)
            == Some(true),
        "Report archive is inaccessible"
    );
    let mut result = Vec::new();
    for fight in report["fights"]
        .as_array()
        .context("Invalid candidate fights")?
    {
        let id = fight["encounterID"]
            .as_i64()
            .context("Invalid candidate encounter")?;
        if fight["inProgress"]
            .as_bool()
            .context("Invalid candidate upload status")?
            || id <= 0
            || encounter.is_some_and(|selected| selected != id)
        {
            continue;
        }
        let level = fight["difficulty"]
            .as_i64()
            .context("Invalid candidate difficulty")?;
        if difficulty.is_some_and(|selected| selected != level) {
            continue;
        }
        let metadata: FightMetadata =
            serde_json::from_value(fight.clone()).context("Invalid candidate metadata")?;
        if game_zone.is_some_and(|id| Some(id) != metadata.zone_id()) {
            continue;
        }
        let planned = PlannedFight {
            report: code.into(),
            fight: fight["id"].as_i64().context("Invalid candidate fight ID")?,
            name: fight["name"]
                .as_str()
                .context("Missing candidate name")?
                .into(),
            encounter: id,
            difficulty: level,
            kill: fight["kill"]
                .as_bool()
                .context("Invalid candidate result")?,
            start_time: fight["startTime"]
                .as_i64()
                .context("Invalid candidate start")?,
            end_time: fight["endTime"].as_i64().context("Invalid candidate end")?,
            metadata,
        };
        planned.validate()?;
        result.push(planned);
    }
    Ok(result)
}

fn ranking_reports(data: &Value) -> Result<(Vec<String>, bool)> {
    let page = data
        .pointer("/worldData/encounter/fightRankings")
        .context("Missing encounter rankings")?;
    let rows = page["rankings"]
        .as_array()
        .context("Invalid encounter rankings")?;
    let reports = rows
        .iter()
        .map(|row| {
            let report = row
                .pointer("/report/code")
                .and_then(Value::as_str)
                .context("Ranking has no report code")?;
            validate_selection(report, None)?;
            Ok(report.to_owned())
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((
        reports,
        page["hasMorePages"]
            .as_bool()
            .context("Missing rankings page status")?,
    ))
}

fn zone_reports(data: &Value) -> Result<(Vec<String>, bool)> {
    let page = data
        .pointer("/reportData/reports")
        .context("Missing zone reports")?;
    let reports = page["data"]
        .as_array()
        .context("Invalid zone reports")?
        .iter()
        .map(|row| {
            let code = row["code"].as_str().context("Missing zone report code")?;
            validate_selection(code, None)?;
            Ok(code.to_owned())
        })
        .collect::<Result<_>>()?;
    Ok((
        reports,
        page["has_more_pages"]
            .as_bool()
            .context("Missing zone page status")?,
    ))
}

impl FFLogsCollectCommand {
    // Keep ranking pagination failures in the plan, e.g. resume after a quota limit.
    fn discover_reports(&self, client: &Client, plan: &mut DownloadPlan) -> Vec<String> {
        let mut reports = Vec::new();
        for page in self.page..self.page + self.pages {
            let found = if let Some(zone) = self.game_zone {
                client
                    .request(ZONE_REPORTS, json!({"zone":zone,"page":page}))
                    .and_then(|data| zone_reports(&data))
            } else {
                client
                    .request(
                        RANKINGS,
                        json!({"encounter":self.encounter,
                "difficulty":self.difficulty, "page":page, "partition":self.partition,
                "region":self.region}),
                    )
                    .and_then(|data| ranking_reports(&data))
            };
            let more = match found {
                Ok((codes, more)) => {
                    reports.extend(codes);
                    plan.truncated |= more && page + 1 == self.page + self.pages;
                    more
                }
                Err(error) => {
                    plan.failures.push(Failure {
                        report: String::new(),
                        fight: None,
                        error: error.to_string(),
                    });
                    break;
                }
            };
            if !more {
                break;
            }
        }
        reports
    }

    fn collect(&self, client: &Client) -> Result<DownloadPlan> {
        let purpose: Purpose = self.purpose.parse()?;
        let mut plan = DownloadPlan {
            schema_version: 3,
            purpose: Some(purpose),
            collected_at_unix_ms: now()?,
            source: PlanSource {
                reports: self.reports.clone(),
                encounter: self.encounter,
                difficulty: self.difficulty,
                game_zone: self.game_zone,
                partition: self.partition,
                region: self.region.clone(),
                first_page: self.page,
                pages: self.pages,
            },
            selection_policy: purpose.policy().into(),
            candidate_count: 0,
            fights: Vec::new(),
            failures: Vec::new(),
            truncated: false,
        };
        let mut reports = if self.reports.is_empty() {
            self.discover_reports(client, &mut plan)
        } else {
            self.reports.clone()
        };
        let mut seen = BTreeSet::new();
        reports.retain(|code| seen.insert(code.clone()));
        plan.truncated |= reports.len() > self.max_reports;
        for code in reports.into_iter().take(self.max_reports) {
            let candidates = client
                .request(CANDIDATES, json!({"code":code}))
                .and_then(|data| {
                    let report = data
                        .pointer("/reportData/report")
                        .context("Report missing or inaccessible")?;
                    ensure!(
                        report["code"].as_str() == Some(&code),
                        "Candidate report code mismatch"
                    );
                    candidate_fights(report, self.encounter, self.difficulty, self.game_zone)
                });
            let failed = candidates.is_err();
            match candidates {
                Ok(fights) => plan.fights.extend(fights),
                Err(error) => {
                    plan.failures.push(Failure {
                        report: code,
                        fight: None,
                        error: error.to_string(),
                    });
                }
            }
            if failed && client.check_budget().is_err() {
                break;
            }
        }
        plan.candidate_count = plan.fights.len();
        plan.truncated |= plan.candidate_count > self.max_fights;
        plan.fights = prioritize(plan.fights, self.max_fights, purpose);
        Ok(plan)
    }
}

impl Run for FFLogsCollectCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        self.purpose.parse::<Purpose>()?;
        self.access.validate()?;
        ensure!(!self.output.exists(), "Download plan already exists");
        ensure!(
            !self.reports.is_empty() || self.encounter.is_some() || self.game_zone.is_some(),
            "Supply report codes, --game-zone, or --encounter"
        );
        ensure!(
            self.encounter
                .is_none_or(|id| id > 0 && id <= i32::MAX as i64)
                && self
                    .difficulty
                    .is_none_or(|id| id >= 0 && id <= i32::MAX as i64)
                && self
                    .partition
                    .is_none_or(|id| id > 0 && id <= i32::MAX as i64),
            "Invalid discovery selection"
        );
        ensure!(
            self.game_zone
                .is_none_or(|id| (1..=i64::from(i32::MAX)).contains(&id)),
            "Invalid game zone"
        );
        ensure!(
            self.game_zone.is_none() || (self.partition.is_none() && self.region.is_none()),
            "--partition and --region apply only to rankings discovery"
        );
        ensure!(
            self.page > 0
                && self.pages > 0
                && self
                    .page
                    .checked_add(self.pages)
                    .is_some_and(|end| end <= i32::MAX as usize)
                && self.max_reports > 0
                && self.max_fights > 0,
            "Invalid discovery limits"
        );
        for code in &self.reports {
            validate_selection(code, None)?;
        }
        let plan = self.collect(&self.access.connect()?)?;
        write_json(&self.output, &plan, false)?;
        tracing::info!(fights = plan.fights.len(), failures = plan.failures.len(), output = %self.output, "Saved FFLogs download plan; no events downloaded");
        ensure!(
            !plan.fights.is_empty() && plan.failures.is_empty(),
            "Incomplete discovery; inspect {}",
            self.output
        );
        plan.validate()
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadReport {
    downloaded: usize,
    skipped: usize,
    bytes: u64,
    remaining: usize,
    failures: Vec<Failure>,
}

fn download(
    plan: &DownloadPlan,
    directory: &Utf8Path,
    limit: usize,
    max_bytes: u64,
    connect: impl FnOnce() -> Result<Client>,
) -> Result<DownloadReport> {
    let mut result = DownloadReport::default();
    let mut client = None;
    let mut connect = Some(connect);
    for (index, fight) in plan.fights.iter().enumerate() {
        let target = directory.join(format!("{}_{}.json", fight.report, fight.fight));
        let outcome = (|| -> Result<bool> {
            if target.exists() {
                let data: CollectedLog = serde_json::from_slice(&fs::read(&target)?)
                    .context("Existing download is invalid; preserve it and choose a different output directory")?;
                fight.verify(&data)?;
                return Ok(false);
            }
            if result.downloaded >= limit {
                return Ok(true);
            }
            let client = match &client {
                Some(client) => client,
                None => client.insert(connect
                    .take()
                    .context("Download connection unavailable")?(
                )?),
            };
            let metadata = client.metadata(&fight.report, Some(fight.fight))?;
            fight.verify_metadata(&metadata)?;
            let data = client.collect_from_report(
                &fight.report,
                fight.fight,
                metadata,
                Some(fight.fight),
            )?;
            fight.verify(&data)?;
            let bytes = serde_json::to_vec(&data)?;
            let size = u64::try_from(bytes.len())?;
            ensure!(
                size <= max_bytes - result.bytes,
                "Download byte budget exhausted; resume with a new budget"
            );
            fs::create_dir_all(directory)?;
            crate::output::write_new(&[(target.as_std_path(), &bytes)])?;
            result.downloaded += 1;
            result.bytes += size;
            Ok(true)
        })();
        match outcome {
            Ok(false) => result.skipped += 1,
            Ok(true) if result.downloaded >= limit => {
                result.remaining = plan.fights.len() - index - 1;
                break;
            }
            Ok(true) => (),
            Err(error) => {
                result.failures.push(Failure {
                    report: fight.report.clone(),
                    fight: Some(fight.fight),
                    error: error.to_string(),
                });
                result.remaining = plan.fights.len() - index;
                break;
            }
        }
    }
    Ok(result)
}

impl Run for FFLogsDownloadCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        self.access.validate()?;
        ensure!(
            self.limit > 0 && self.max_bytes > 0,
            "Invalid download limits"
        );
        let plan: DownloadPlan =
            serde_json::from_slice(&fs::read(&self.plan)?).context("Invalid download plan JSON")?;
        plan.validate()?;
        // Keep status outside the log directory, where input discovery expects only raw fight JSON.
        fs::create_dir_all(&self.output)?;
        let plan_path = Utf8PathBuf::from_path_buf(fs::canonicalize(&self.plan)?)
            .map_err(|_path| anyhow::anyhow!("Download plan path is not UTF-8"))?;
        let report_path = plan_path.with_extension("download.json");
        let output_path = fs::canonicalize(&self.output)?;
        ensure!(
            !plan_path.as_std_path().starts_with(&output_path)
                && !report_path.as_std_path().starts_with(&output_path),
            "Download plan/status must be outside the log output directory"
        );
        let report = download(&plan, &self.output, self.limit, self.max_bytes, || {
            self.access.connect()
        })?;
        write_json(&report_path, &report, true)?;
        tracing::info!(downloaded = report.downloaded, skipped = report.skipped, remaining = report.remaining,
            failures = report.failures.len(), report = %report_path, "FFLogs download completed");
        ensure!(
            report.failures.is_empty(),
            "Download stopped; inspect {} and rerun the same command to resume",
            report_path
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests;

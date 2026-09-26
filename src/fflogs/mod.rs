mod client;
mod storage;

use std::{
    collections::BTreeSet,
    io::{self, Write},
};

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use serde_json::Value;
use usage::{Args, Run, ValueEnum};

use client::{API_URL, Client, TOKEN_URL};
use storage::save;

#[derive(Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    JsonPretty,
}

// Avoid Debug so credentials cannot appear in diagnostic output.
#[derive(Args)]
pub struct FFLogsCommand {
    /// FFLogs report code
    #[usage(arg)]
    report_code: String,
    /// Fight ID; with --all, selects all pulls of the same encounter and difficulty
    #[usage(arg)]
    fight_id: Option<i64>,
    /// List fights without downloading events
    #[usage(long)]
    list: bool,
    /// Download all completed pulls of one encounter, including wipes
    #[usage(long)]
    all: bool,
    /// Match a fight name (case-insensitive substring); use with --list or --all
    #[usage(long)]
    name: Option<String>,
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

// Infer batch identity from visible fights; never require the user to know an encounter ID.
fn select_fights(
    report: &Value,
    seed: Option<i64>,
    name: Option<&str>,
    all: bool,
) -> Result<Vec<i64>> {
    let fights = report
        .get("fights")
        .and_then(Value::as_array)
        .context("Invalid fights array")?;
    if !all {
        let id = seed.context("Supply a fight ID, --list, or --all")?;
        ensure!(
            fights.iter().any(|f| f["id"].as_i64() == Some(id)),
            "Fight {id} not found"
        );
        return Ok(vec![id]);
    }
    // Normalize the search once, e.g. "LINDWURM" matches every candidate's lowercase name.
    let name = name.map(str::to_lowercase);
    let groups: BTreeSet<_> = fights
        .iter()
        .filter(|f| {
            seed.is_none_or(|id| f["id"].as_i64() == Some(id))
                && name.as_deref().is_none_or(|n| {
                    f["name"]
                        .as_str()
                        .is_some_and(|v| v.to_lowercase().contains(n))
                })
                && f["encounterID"].as_i64().is_some_and(|id| id > 0)
        })
        .map(|f| (f["encounterID"].as_i64(), f["difficulty"].as_i64()))
        .collect();
    ensure!(
        groups.len() == 1,
        "Batch selection must identify one encounter and difficulty; use --list, then a fight ID with --all or a more specific --name"
    );
    let mut ids = BTreeSet::new();
    for fight in fights {
        if groups.contains(&(fight["encounterID"].as_i64(), fight["difficulty"].as_i64()))
            && fight["inProgress"].as_bool() == Some(false)
        {
            ids.insert(fight["id"].as_i64().context("Invalid fight ID")?);
        }
    }
    ensure!(!ids.is_empty(), "No completed fights selected");
    Ok(ids.into_iter().collect())
}

fn list_fights(report: &Value, name: Option<&str>, writer: &mut impl Write) -> Result<()> {
    let name = name.map(str::to_lowercase);
    writeln!(
        writer,
        "| Fight | Name | Difficulty | Result | Length (s) | Progress (%) | Boss HP (%) | Last phase |"
    )?;
    writeln!(writer, "|---:|---|---:|---|---:|---:|---:|---:|")?;
    for fight in report["fights"]
        .as_array()
        .context("Invalid fights array")?
    {
        let title = fight["name"].as_str().unwrap_or("Unknown");
        if name
            .as_deref()
            .is_some_and(|n| !title.to_lowercase().contains(n))
        {
            continue;
        }
        let result = if fight["inProgress"].as_bool() == Some(true) {
            "uploading"
        } else if fight["kill"].as_bool() == Some(true) {
            "kill"
        } else {
            "wipe"
        };
        let length = (fight["endTime"].as_f64().context("Invalid fight endTime")?
            - fight["startTime"]
                .as_f64()
                .context("Invalid fight startTime")?)
            / 1000.0;
        writeln!(
            writer,
            "| {} | {} | {} | {} | {:.1} | {} | {} | {} |",
            fight["id"],
            title.replace(['\n', '\r'], " ").replace('|', "\\|"),
            fight["difficulty"],
            result,
            length,
            fight["fightPercentage"],
            fight["bossPercentage"],
            fight["lastPhase"]
        )?;
    }
    Ok(())
}

impl FFLogsCommand {
    // Reject invalid options before authentication, e.g. --list --all must not contact FFLogs.
    fn validate(&self) -> Result<()> {
        ensure!(
            !(self.list && self.all),
            "--list and --all cannot be combined"
        );
        ensure!(
            !self.list || self.fight_id.is_none(),
            "--list does not take a fight ID"
        );
        ensure!(
            self.name.is_none() || self.list || self.all,
            "--name requires --list or --all"
        );
        ensure!(
            self.name.as_ref().is_none_or(|n| !n.trim().is_empty()),
            "Empty fight name"
        );
        ensure!(
            self.list || self.all || self.fight_id.is_some(),
            "Supply a fight ID, --list, or --all"
        );
        client::validate_selection(&self.report_code, self.fight_id)?;
        ensure!(
            !self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty(),
            "Empty FFLogs credentials"
        );
        Ok(())
    }
}

impl Run for FFLogsCommand {
    type Output = Result<()>;
    fn run(self) -> Result<()> {
        self.validate()?;
        let client =
            Client::authenticate(TOKEN_URL, API_URL, &self.client_id, &self.client_secret)?;
        let metadata_fight_id = if self.all || self.list {
            None
        } else {
            self.fight_id
        };
        let report = client.metadata(&self.report_code, metadata_fight_id)?;
        if self.list {
            return list_fights(&report, self.name.as_deref(), &mut io::stdout().lock());
        }
        let ids = select_fights(&report, self.fight_id, self.name.as_deref(), self.all)?;
        let directory = self.output.unwrap_or_default();
        // Save each completed pull immediately; a later failure leaves earlier pulls intact.
        for id in ids {
            let data = client
                .collect_from_report(&self.report_code, id, report.clone(), metadata_fight_id)
                .with_context(|| format!("Failed to collect fight {id}"))?;
            save(
                &data,
                &directory,
                &format!("{}_{}.json", self.report_code, id),
                self.format,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

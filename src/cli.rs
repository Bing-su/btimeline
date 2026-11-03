use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use serde::Deserialize;
use usage_rs::{Args, Cli, Run, Subcommands};

use crate::fflogs::{FFLogsCollectCommand, FFLogsCommand, FFLogsDownloadCommand};

#[derive(Subcommands)]
#[usage(run)]
pub enum MainCommands {
    /// Download fights from an FFLogs report.
    Fflogs(FFLogsCommand),
    /// Discover fights and save a download plan.
    FflogsCollect(FFLogsCollectCommand),
    /// Download fights from a saved plan.
    FflogsDownload(FFLogsDownloadCommand),
    /// Inspect collected FFLogs inputs.
    Inspect(InspectCommand),
    /// Mine observed event sequences into a KDL process graph.
    Mine(MineCommand),
    /// Analyze collected FFLogs fights and generate a KDL timeline.
    Generate(GenerateCommand),
    /// Check timeline structure and authored constraints.
    Validate(ValidateCommand),
    /// Replay normalized recorded signals against a timeline.
    Compare(CompareCommand),
    /// Query upcoming visible events using observations only through a cutoff.
    Forecast(ForecastCommand),
    /// Evaluate an authored timeline against FFLogs inputs and an explicit projection profile.
    Evaluate(crate::evaluate::EvaluateCommand),
    /// Show a bounded raw cast window with original event indices.
    Evidence(crate::evaluate::EvidenceCommand),
    /// Summarize a saved evaluation with bounded failure and unclassified examples.
    Review(crate::evaluate::ReviewCommand),
}

#[derive(Args)]
pub struct ValidateCommand {
    #[usage(arg)]
    timeline: Utf8PathBuf,
    /// Print definition IDs and ownership for authoring profile bindings.
    #[usage(long)]
    json: bool,
}

impl Run for ValidateCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let text = std::fs::read_to_string(&self.timeline)?;
        let model = crate::timeline::parse(&text)
            .with_context(|| format!("Validating {}", self.timeline))?;
        if self.json {
            println!(
                "{}",
                serde_json::to_string(
                    &serde_json::json!({"name":model.name,"catalog":model.catalog})
                )?
            );
            return Ok(());
        }
        println!(
            "Valid structure: {} ({} events)",
            model.name, model.event_count
        );
        println!(
            "Not checked: log type/field catalog, conditional overlaps, dynamic execution constraints, game accuracy."
        );
        Ok(())
    }
}

#[derive(Args)]
pub struct CompareCommand {
    #[usage(arg)]
    timeline: Utf8PathBuf,
    /// Normalized signal recordings or collected FFLogs fights; each file is an independent replay.
    #[usage(arg)]
    recordings: Vec<Utf8PathBuf>,
    /// Print replay records as JSON, retaining raw millisecond timestamps.
    #[usage(long)]
    json: bool,
}

impl Run for CompareCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        anyhow::ensure!(
            !self.recordings.is_empty(),
            "Provide at least one normalized signal recording"
        );
        let model = crate::timeline::parse(&std::fs::read_to_string(&self.timeline)?)
            .with_context(|| format!("Validating {}", self.timeline))?;
        let mut results = std::collections::BTreeMap::new();
        let mut summaries = std::collections::BTreeMap::new();
        let mut has_fflogs = false;
        for path in &self.recordings {
            let bytes = std::fs::read(path).with_context(|| format!("Reading recording {path}"))?;
            let shape: RecordingShape = serde_json::from_slice(&bytes)
                .with_context(|| format!("Reading recording {path}"))?;
            let recording: crate::timeline::Recording =
                if shape.report.is_some() || shape.collection.is_some() {
                    has_fflogs = true;
                    let log = crate::generate::validated_collected(path.as_std_path(), &bytes)
                        .with_context(|| format!("Validating FFLogs input {path}"))?;
                    crate::timeline::from_fflogs(&log)
                        .with_context(|| format!("Reading FFLogs input {path}"))?
                } else {
                    serde_json::from_slice(&bytes)
                        .with_context(|| format!("Reading recording {path}"))?
                };
            let records = crate::timeline::replay(&model, &recording)
                .with_context(|| format!("Replaying {path}"))?;
            summaries.insert(path.as_str(), recording.summary(&records));
            results.insert(path.as_str(), records);
        }
        let mut not_checked = vec!["missing-signal coverage", "game accuracy"];
        if has_fflogs {
            not_checked.push("FFLogs-to-ACT signal equivalence");
        }
        if self.json {
            serde_json::to_writer_pretty(
                std::io::stdout().lock(),
                &serde_json::json!({
                    "recordings": results,
                    "summaries": summaries,
                    "not_checked": not_checked
                }),
            )?;
            println!();
            return Ok(());
        }
        for (path, records) in results {
            println!("{path}");
            if let Some(summary) = summaries.get(path) {
                println!("  {summary}");
            }
            for record in records {
                println!(
                    "  {:.1}s [run {}] {} {}",
                    record.at_ms as f64 / 1000.0,
                    record.run,
                    record.action,
                    record.detail
                );
            }
        }
        println!("Replay only: {} not checked.", not_checked.join(", "));
        Ok(())
    }
}

#[derive(Args)]
pub struct ForecastCommand {
    #[usage(arg)]
    timeline: Utf8PathBuf,
    /// Normalized signal recording; future observations are ignored by the forecast.
    #[usage(arg)]
    recording: Utf8PathBuf,
    /// Query cutoff in raw milliseconds, including observations at that time.
    #[usage(long)]
    at_ms: i64,
    /// Positive lookahead duration in milliseconds.
    #[usage(long)]
    horizon_ms: i64,
}

impl Run for ForecastCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let model = crate::timeline::parse(&std::fs::read_to_string(&self.timeline)?)
            .with_context(|| format!("Validating {}", self.timeline))?;
        let input = serde_json::from_slice(&std::fs::read(&self.recording)?)
            .with_context(|| format!("Reading normalized recording {}", self.recording))?;
        let forecast = crate::timeline::forecast(&model, &input, self.at_ms, self.horizon_ms)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &forecast)?;
        println!();
        Ok(())
    }
}

#[derive(Deserialize)]
struct RecordingShape {
    report: Option<serde::de::IgnoredAny>,
    collection: Option<serde::de::IgnoredAny>,
}

#[derive(Args)]
pub struct MineCommand {
    /// Collected fights from one encounter and difficulty.
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
    /// New KDL file for the observed process graph.
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}

impl Run for MineCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        crate::generate::mine(&self.inputs, self.output.as_std_path())?;
        eprintln!("Process graph saved to {}", self.output);
        Ok(())
    }
}

#[derive(Args)]
pub struct GenerateCommand {
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
    /// Content mode: raid, dungeon, or alliance.
    #[usage(long)]
    mode: String,
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}

impl Run for GenerateCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let count =
            crate::generate::generate(&self.inputs, self.mode.parse()?, self.output.as_std_path())?;
        println!("Generated {} ({} event definitions).", self.output, count);
        println!("Evidence: {}", self.output.with_extension("evidence.json"));
        println!("Review the generation evidence for unsupported paths and excluded observations.");
        Ok(())
    }
}

#[derive(Args)]
pub struct InspectCommand {
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
    /// Print detailed observations and provenance as JSON instead of a summary.
    #[usage(long)]
    json: bool,
}

impl Run for InspectCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let groups = crate::generate::inspect(&self.inputs)?;
        if self.json {
            let mut value = serde_json::to_value(&groups)?;
            crate::time::json_times(&mut value, true)?;
            serde_json::to_writer_pretty(std::io::stdout().lock(), &value)?;
            println!();
            return Ok(());
        }
        for group in &groups {
            let clears = group.pulls.iter().filter(|pull| pull.kill).count();
            println!(
                "Encounter {} (difficulty {}): fights={}, clears={}",
                group.key.encounter,
                group.key.difficulty,
                group.pulls.len(),
                clears
            );
            for pull in &group.pulls {
                println!(
                    "  {}/{} {}: {:.1}s, {} events, {} cast observations, {} reported phase boundaries",
                    pull.report,
                    pull.fight,
                    pull.name,
                    pull.end_ms as f64 / 1000.0,
                    pull.event_count,
                    pull.occurrences.len(),
                    pull.phase_transitions.len()
                );
            }
            if clears < 2 {
                println!(
                    "  Single or matching logs can generate a representative-route draft; divergent routes still need two clears."
                );
            }
        }
        println!(
            "Next: btimeline generate LOG... --mode MODE -o timeline.kdl (MODE: raid, dungeon, alliance)."
        );
        Ok(())
    }
}

#[derive(Cli)]
#[usage(bin = "btimeline", version = env!("CARGO_PKG_VERSION"), completion)]
pub struct MainCli {
    #[usage(subcommand)]
    pub command: MainCommands,
}

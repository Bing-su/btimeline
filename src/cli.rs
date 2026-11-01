use anyhow::{Context, Result};
use camino::Utf8PathBuf;
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
    /// Check timeline structure and authored constraints.
    Validate(ValidateCommand),
    /// Replay normalized recorded signals against a timeline.
    Compare(CompareCommand),
}

#[derive(Args)]
pub struct ValidateCommand {
    #[usage(arg)]
    timeline: Utf8PathBuf,
}

impl Run for ValidateCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let text = std::fs::read_to_string(&self.timeline)?;
        let model = crate::timeline::parse(&text)
            .with_context(|| format!("Validating {}", self.timeline))?;
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
    /// Normalized signal recordings; each file is an independent replay.
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
        for path in &self.recordings {
            let recording: crate::timeline::Recording =
                serde_json::from_slice(&std::fs::read(path)?)
                    .with_context(|| format!("Reading recording {path}"))?;
            results.insert(
                path.as_str(),
                crate::timeline::replay(&model, &recording)
                    .with_context(|| format!("Replaying {path}"))?,
            );
        }
        if self.json {
            serde_json::to_writer_pretty(
                std::io::stdout().lock(),
                &serde_json::json!({
                    "recordings": results,
                    "not_checked": ["missing-signal coverage", "game accuracy"]
                }),
            )?;
            println!();
        } else {
            for (path, records) in results {
                println!("{path}");
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
            println!("Replay only: missing-signal coverage and game accuracy are not checked.");
        }
        Ok(())
    }
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
pub struct InspectCommand {
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
}

impl Run for InspectCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let mut groups = serde_json::to_value(crate::generate::inspect(&self.inputs)?)?;
        crate::time::json_times(&mut groups, true)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &groups)?;
        println!();
        Ok(())
    }
}

#[derive(Cli)]
#[usage(bin = "btimeline", version = env!("CARGO_PKG_VERSION"), completion)]
pub struct MainCli {
    #[usage(subcommand)]
    pub command: MainCommands,
}

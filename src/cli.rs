use anyhow::Result;
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

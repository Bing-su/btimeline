use usage::{Cli, Subcommands};

use crate::fflogs::FFLogsCommand;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Subcommands)]
#[usage(run)]
pub enum MainCommands {
    Fflogs(FFLogsCommand),
}

#[derive(Debug, Cli)]
#[usage(bin = "btimeline", version = VERSION, completion)]
pub struct MainCli {
    #[usage(subcommand)]
    pub command: MainCommands,
}

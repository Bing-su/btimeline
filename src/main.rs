mod cli;
mod fflogs;

use anyhow::Result;
use usage::Run;

use crate::cli::MainCli;

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    MainCli::parse().command.run()
}

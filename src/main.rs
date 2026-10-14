mod cli;
mod fflogs;
mod generate;
mod output;
mod timeline;

use anyhow::Result;
use usage_rs::Run;

use crate::cli::MainCli;

fn main() -> Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    MainCli::parse().command.run()
}

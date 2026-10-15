mod cli;
mod fflogs;
mod generate;
mod output;
mod timeline;

use anyhow::Result;
use usage_rs::Run;

use crate::cli::MainCli;

fn main() -> Result<()> {
    // Load before any workers start; exported variables still override local development credentials.
    if let Err(error) = dotenvy::from_path(".env")
        && !matches!(&error, dotenvy::Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound)
    {
        anyhow::bail!("Cannot load .env; check its syntax and permissions");
    }
    tracing_subscriber::fmt().with_target(false).init();
    MainCli::parse().command.run()
}

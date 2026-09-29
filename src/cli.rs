use camino::Utf8PathBuf;

use anyhow::Result;
use usage::{Args, Cli, Run, Subcommands};

use crate::fflogs::FFLogsCommand;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Subcommands)]
#[usage(run)]
pub enum MainCommands {
    Fflogs(FFLogsCommand),
    Validate(ValidateCommand),
    Convert(ConvertCommand),
    ExportSchema(ExportSchemaCommand),
    InspectInputs(InspectInputsCommand),
    Generate(GenerateCommand),
    ReportMarkdown(ReportMarkdownCommand),
}

#[derive(Cli)]
#[usage(bin = "btimeline", version = VERSION, completion)]
pub struct MainCli {
    #[usage(subcommand)]
    pub command: MainCommands,
}

#[derive(Args)]
pub struct ValidateCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
}
impl Run for ValidateCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        crate::timeline::validate_file(self.input)?;
        tracing::info!(
            "[Validate] Schema and semantic validation passed; cactbot parser/runtime not checked"
        );
        Ok(())
    }
}

#[derive(Args)]
pub struct ConvertCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}
impl Run for ConvertCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        crate::timeline::convert_file(self.input, self.output)
    }
}

#[derive(Args)]
pub struct ExportSchemaCommand {
    #[usage(short = 'o', long, default = "schema/btimeline-v1.schema.json")]
    output: Utf8PathBuf,
}
impl Run for ExportSchemaCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        crate::timeline::export_schema(self.output)
    }
}

#[derive(Args)]
pub struct InspectInputsCommand {
    /// Collected FFLogs fight JSON files
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
}
impl Run for InspectInputsCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        let groups = crate::generate::inspect(&self.inputs)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &groups)?;
        println!();
        Ok(())
    }
}

#[derive(Args)]
pub struct GenerateCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}
impl Run for GenerateCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        crate::generate::generate(self.input, self.output)
    }
}

#[derive(Args)]
pub struct ReportMarkdownCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
}
impl Run for ReportMarkdownCommand {
    type Output = Result<()>;
    fn run(self) -> Self::Output {
        crate::generate::markdown_file(self.input)
    }
}

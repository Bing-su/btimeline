use anyhow::{Result, ensure};
use camino::Utf8PathBuf;
use usage::{Args, Cli, Run, Subcommands};

use crate::fflogs::FFLogsCommand;
use crate::generate::GenerateMode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Subcommands)]
#[usage(run)]
pub enum MainCommands {
    Fflogs(FFLogsCommand),
    Validate(ValidateCommand),
    Convert(ConvertCommand),
    ExportSchema(ExportSchemaCommand),
    InspectInputs(InspectInputsCommand),
    Align(AlignCommand),
    Generate(GenerateCommand),
    /// Generate a YAML draft, validate it, and replay its input logs, e.g. prepare logs/fight -o out/fight.yaml.
    Prepare(PrepareCommand),
    ReportMarkdown(ReportMarkdownCommand),
    Replay(ReplayCommand),
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
pub struct AlignCommand {
    /// Collected FFLogs fight JSON files from one compatible group
    #[usage(arg)]
    inputs: Vec<Utf8PathBuf>,
}
impl Run for AlignCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let report = crate::generate::align(&self.inputs)?;
        serde_json::to_writer_pretty(std::io::stdout().lock(), &report)?;
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
    /// Generation mode: raid keeps the full fight; dungeon/alliance keep boss encounters and area resets.
    #[usage(long, value_enum, default = "raid")]
    mode: GenerateMode,
    /// Select one compatible group by its exact fight name, e.g. --name "Example Fight".
    #[usage(long)]
    name: Option<String>,
    /// Select an encounter segment when names overlap, e.g. --encounter 105 for a checkpoint entry.
    #[usage(long)]
    encounter: Option<i64>,
    /// Select the difficulty when an encounter has multiple groups.
    #[usage(long)]
    difficulty: Option<i64>,
    /// Independent preview horizon used to separate virtual blocks, e.g. --lookahead 30.
    #[usage(long, default = "30")]
    lookahead: f64,
}
impl Run for GenerateCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        crate::generate::generate_selected(
            self.input,
            self.output,
            self.mode,
            self.name.as_deref(),
            self.encounter,
            self.difficulty,
            self.lookahead,
        )
    }
}

#[derive(Args)]
pub struct PrepareCommand {
    #[usage(flatten)]
    generate: GenerateCommand,
    /// Also convert the validated draft to sibling cactbot text, e.g. out/fight.txt (default: false).
    #[usage(long)]
    convert: bool,
}
impl Run for PrepareCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        let timeline = self.generate.output.clone();
        let input = self.generate.input.clone();
        let replay = timeline.with_extension("replay.json");
        let text = timeline.with_extension("txt");
        // Check later outputs before generation, e.g. an existing replay must not leave a new draft.
        for path in [&replay, &replay.with_extension("md")] {
            ensure!(!path.exists(), "Output already exists: {path}");
        }
        if self.convert {
            ensure!(
                timeline != text,
                "YAML and text outputs must differ; use -o draft.yaml"
            );
            ensure!(!text.exists(), "Output already exists: {text}");
        }
        self.generate.run()?;
        crate::timeline::validate_file(&timeline)?;
        crate::generate::replay::replay_file(&timeline, input, &replay, None)?;
        if self.convert {
            crate::timeline::convert_file(&timeline, text)?;
        }
        tracing::info!(
            "[Prepare] YAML validation and input replay passed; cactbot parser/runtime not checked"
        );
        Ok(())
    }
}

#[derive(Args)]
pub struct ReportMarkdownCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
}

#[derive(Args)]
pub struct ReplayCommand {
    /// Generated YAML timeline, e.g. out/fight.yaml.
    #[usage(arg)]
    timeline: Utf8PathBuf,
    /// One collected fight or a directory of compatible pulls.
    #[usage(arg)]
    input: Utf8PathBuf,
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
    /// Generation evidence; defaults to the YAML's .report.json sibling.
    #[usage(long)]
    report: Option<Utf8PathBuf>,
}
impl Run for ReplayCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        crate::generate::replay::replay_file(
            self.timeline,
            self.input,
            self.output,
            self.report.as_deref().map(|path| path.as_std_path()),
        )
    }
}
impl Run for ReportMarkdownCommand {
    type Output = Result<()>;

    fn run(self) -> Self::Output {
        crate::generate::markdown_file(self.input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_defaults_to_raid_and_accepts_boss_modes() {
        for (mode, expected) in [
            (None, GenerateMode::Raid),
            (Some("raid"), GenerateMode::Raid),
            (Some("dungeon"), GenerateMode::Dungeon),
            (Some("alliance"), GenerateMode::Alliance),
        ] {
            let mut args = vec!["generate", "input.json", "-o", "out.yaml"];
            if let Some(mode) = mode {
                args.extend(["--mode", mode]);
            }
            let args: Vec<_> = args.into_iter().map(std::ffi::OsStr::new).collect();
            let MainCommands::Generate(command) = MainCli::parse_from(&args).unwrap().command
            else {
                panic!("expected generate command");
            };
            assert_eq!(command.mode, expected);
        }
    }

    #[test]
    fn generate_accepts_directory_name_selection() {
        let args: Vec<_> = [
            "generate",
            "logs/example",
            "--name",
            "Example Fight",
            "-o",
            "out.yaml",
        ]
        .into_iter()
        .map(std::ffi::OsStr::new)
        .collect();
        let MainCommands::Generate(command) = MainCli::parse_from(&args).unwrap().command else {
            panic!("expected generate command");
        };
        assert_eq!(command.name.as_deref(), Some("Example Fight"));
        assert_eq!(command.input.as_str(), "logs/example");
    }

    #[test]
    fn generate_accepts_encounter_and_difficulty_selection() {
        let args: Vec<_> = [
            "generate",
            "logs/example",
            "--encounter",
            "105",
            "--difficulty",
            "101",
            "-o",
            "out.yaml",
        ]
        .into_iter()
        .map(std::ffi::OsStr::new)
        .collect();
        let MainCommands::Generate(command) = MainCli::parse_from(&args).unwrap().command else {
            panic!("expected generate command");
        };
        assert_eq!(command.encounter, Some(105));
        assert_eq!(command.difficulty, Some(101));
        assert_eq!(command.name, None);
    }
}

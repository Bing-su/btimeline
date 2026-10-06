use std::fs;
use std::path::Path;

use anyhow::{Result, ensure};
use serde::Serialize;

use super::OutputFormat;

// A same-directory temporary file makes replacement atomic, e.g. failed saves retain the old JSON.
pub(super) fn save<T: Serialize>(
    data: &T,
    directory: impl AsRef<Path>,
    filename: &str,
    format: OutputFormat,
) -> Result<()> {
    let dir = directory.as_ref();
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    fs::create_dir_all(dir)?;
    let target = dir.join(filename);
    ensure!(!target.is_dir(), "Output path is a directory");
    crate::output::write_replace(&target, |file| {
        match format {
            OutputFormat::Json => serde_json::to_writer(file, data)?,
            OutputFormat::JsonPretty => serde_json::to_writer_pretty(file, data)?,
        }
        Ok(())
    })
}

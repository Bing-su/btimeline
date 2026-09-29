use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

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
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let dir = directory.as_ref();
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    fs::create_dir_all(dir)?;
    let target = dir.join(filename);
    ensure!(!target.is_dir(), "Output path is a directory");
    let temp = dir.join(format!(
        ".{filename}.{}.{}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        match format {
            OutputFormat::Json => serde_json::to_writer(&mut file, data)?,
            OutputFormat::JsonPretty => serde_json::to_writer_pretty(&mut file, data)?,
        }
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        {
            #![allow(
                clippy::let_underscore_must_use,
                reason = "intentional ignore of remove_file result"
            )]
            let _ = fs::remove_file(&temp);
        }
    }
    result
}

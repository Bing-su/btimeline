use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use tempfile::NamedTempFile;

fn stage(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<NamedTempFile> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut builder = tempfile::Builder::new();
    // Preserve ordinary output permissions under the process umask, e.g. 0644 with umask 022.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o666));
    }
    let mut temp = builder.prefix(".btimeline-").tempfile_in(parent)?;
    write(temp.as_file_mut())?;
    temp.as_file_mut().flush()?;
    temp.as_file().sync_all()?;
    Ok(temp)
}

// Stage everything before publishing; e.g. a blocked report rolls back a newly published YAML.
// Refuse concurrent destination creation; this is not a multi-file atomic transaction.
pub(crate) fn write_new(files: &[(&Path, &[u8])]) -> Result<()> {
    let temps = files
        .iter()
        .map(|(path, bytes)| stage(path, |file| Ok(file.write_all(bytes)?)))
        .collect::<Result<Vec<_>>>()?;
    for (index, ((path, _), temp)) in files.iter().zip(temps).enumerate() {
        if let Err(error) = temp.persist_noclobber(path) {
            for (created, _) in files.iter().take(index) {
                drop(fs::remove_file(created));
            }
            return Err(error.error).with_context(|| format!("Creating {}", path.display()));
        }
    }
    Ok(())
}

// Replace only after serialization and sync succeed, e.g. a failed FFLogs save preserves the old pull.
pub(crate) fn write_replace(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    stage(path, write)?
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_staging_and_publication_preserve_existing_work() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("draft.yaml");
        let report = directory.path().join("draft.report.json");
        let missing = directory.path().join("missing/draft.report.md");
        fs::write(&report, "previous").unwrap();

        // Fail during staging, then during publication after the first file has been linked.
        for blocked in [&missing, &report] {
            assert!(write_new(&[(&first, b"draft"), (blocked, b"report")]).is_err());
            assert!(!first.exists());
            assert_eq!(fs::read_to_string(&report).unwrap(), "previous");
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        }

        assert!(
            write_replace(&report, |file| {
                file.write_all(b"partial")?;
                anyhow::bail!("serialization failed")
            })
            .is_err()
        );
        assert_eq!(fs::read_to_string(&report).unwrap(), "previous");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);

        fs::remove_file(&report).unwrap();
        write_new(&[(&first, b"draft"), (&report, b"report")]).unwrap();
        assert_eq!(fs::read_to_string(&first).unwrap(), "draft");
        assert_eq!(fs::read_to_string(&report).unwrap(), "report");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}

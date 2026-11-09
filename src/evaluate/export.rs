use std::fs;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use serde_json::json;
use usage_rs::{Args, Run};

use super::{Profile, ensure_unchanged, hash, projection};

#[derive(Args)]
pub struct ProjectCommand {
    #[usage(arg)]
    timeline: Utf8PathBuf,
    #[usage(arg)]
    input: Utf8PathBuf,
    /// Use the same scope and aliases as authored evaluation; infer from KDL when omitted.
    #[usage(long)]
    profile: Option<Utf8PathBuf>,
    /// New provenance envelope with a versioned recording; never overwrite an existing result.
    #[usage(short = 'o', long)]
    output: Utf8PathBuf,
}

impl Run for ProjectCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        ensure!(
            !self.output.exists(),
            "Projection already exists: {}",
            self.output
        );
        let timeline_bytes = fs::read(&self.timeline)?;
        let input_bytes = fs::read(&self.input)?;
        let model = crate::timeline::parse(std::str::from_utf8(&timeline_bytes)?)?;
        let profile_bytes = self.profile.as_ref().map(fs::read).transpose()?;
        let profile = if let Some(bytes) = &profile_bytes {
            serde_json::from_slice::<Profile>(bytes)?.resolve(&model)?
        } else {
            Profile::infer(&model).context("No supported KDL entry; provide --profile")?
        };
        profile.validate()?;
        profile.validate_bindings(&model)?;
        let mut log = crate::generate::validated_collected(self.input.as_std_path(), &input_bytes)?;
        profile.apply_source_aliases(&mut log);
        let projected = projection::project(&log, &profile, Some(&model))?;
        let result = json!({
            "version": 1,
            "timeline_sha256": hash(&timeline_bytes),
            "input_sha256": hash(&input_bytes),
            "profile_sha256": profile_bytes.as_ref().map(|bytes| hash(bytes)),
            "report": log.report.code,
            "fight": log.report.fights.first().context("Missing fight")?.id,
            "origin_timestamp_ms": projected.origin_timestamp_ms,
            "recording": projected.recording,
            "projection_profile": profile_bytes.as_ref().map(|bytes| serde_json::from_slice::<serde_json::Value>(bytes)).transpose()?,
            "assumptions": ["FFLogs cast becomes Ability; begincast becomes StartsUsing only under the declared profile", "No synthetic lifecycle signals; aliases apply only to the projection"],
            "not_checked": ["ACT packet equivalence", "game accuracy", "UI rendering"]
        });
        ensure_unchanged(self.timeline.as_str(), &timeline_bytes)?;
        ensure_unchanged(self.input.as_str(), &input_bytes)?;
        if let Some((path, bytes)) = self.profile.as_ref().zip(profile_bytes.as_ref()) {
            ensure_unchanged(path.as_str(), bytes)?;
        }
        crate::output::write_new(&[(
            self.output.as_std_path(),
            &serde_json::to_vec_pretty(&result)?,
        )])?;
        println!(
            "{}",
            json!({"output":self.output.as_str(),"observations":projected.recording.observations.len()})
        );
        Ok(())
    }
}

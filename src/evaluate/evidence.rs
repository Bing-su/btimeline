use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result, ensure};
use camino::Utf8PathBuf;
use serde_json::json;
use usage_rs::{Args, Run};

fn window(
    log: &crate::fflogs::model::CollectedLog,
    index: Option<usize>,
    before: i64,
    after: i64,
) -> Result<Option<(i64, i64)>> {
    let center = index
        .map(|index| {
            log.events
                .get(index)
                .map(|event| event.timestamp)
                .context("Evidence raw index out of bounds")
        })
        .transpose()?;
    center
        .map(|at| {
            Ok((
                at.checked_sub(before).context("Evidence window overflow")?,
                at.checked_add(after).context("Evidence window overflow")?,
            ))
        })
        .transpose()
}

#[derive(Args)]
pub struct EvidenceCommand {
    #[usage(arg)]
    input: Utf8PathBuf,
    /// Center a time window on this original raw.events index, including non-cast events.
    #[usage(long)]
    around_index: Option<usize>,
    #[usage(long, default = "5000")]
    before_ms: i64,
    #[usage(long, default = "5000")]
    after_ms: i64,
    /// Exact source name; without it, show NPC casts, including helpers absent from enemyNPCs.
    #[usage(long)]
    source: Option<String>,
    /// Maximum output rows (1..200); omitted matches are counted.
    #[usage(long, default = "40")]
    limit: usize,
}
impl Run for EvidenceCommand {
    type Output = Result<()>;

    fn run(self) -> Result<()> {
        ensure!(
            (1..=200).contains(&self.limit),
            "Evidence limit must be in 1..200"
        );
        ensure!(
            (0..=crate::timeline::MAX_MS).contains(&self.before_ms)
                && (0..=crate::timeline::MAX_MS).contains(&self.after_ms),
            "Invalid evidence window"
        );
        ensure!(
            self.around_index.is_some() || (self.before_ms == 5000 && self.after_ms == 5000),
            "Time window requires --around-index"
        );
        let bytes = fs::read(&self.input)?;
        let log = crate::generate::validated_collected(self.input.as_std_path(), &bytes)?;
        let fight = log.report.fights.first().context("Missing fight")?;
        let window = window(&log, self.around_index, self.before_ms, self.after_ms)?;
        let actors = log
            .report
            .master_data
            .actors
            .iter()
            .map(|a| (a.id, a))
            .collect::<BTreeMap<_, _>>();
        let abilities = log
            .report
            .master_data
            .abilities
            .iter()
            .map(|a| (a.game_id, a.name.as_str()))
            .collect::<BTreeMap<_, _>>();
        let mut matches = log
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                if !matches!(event.kind.as_str(), "cast" | "begincast")
                    || window.is_some_and(|(start, end)| {
                        event.timestamp < start || event.timestamp > end
                    })
                {
                    return false;
                }
                let actor = event.source_id.and_then(|id| actors.get(&id));
                match &self.source {
                    Some(source) => actor.is_some_and(|actor| &actor.name == source),
                    None => actor.is_some_and(|actor| actor.kind == "NPC"),
                }
            })
            .collect::<Vec<_>>();
        let total_matches = matches.len();
        // Dense packets before the anchor must not push the requested event out of a small context window.
        if let Some(index) = self.around_index {
            let center = log
                .events
                .get(index)
                .context("Evidence raw index out of bounds")?
                .timestamp;
            matches.sort_by_key(|(raw_index, event)| {
                (event.timestamp.abs_diff(center), raw_index.abs_diff(index))
            });
        }
        matches.truncate(self.limit);
        // Present the selected view chronologically, preserving raw indices and equal-time packet order.
        matches.sort_by_key(|(index, event)| (event.timestamp, *index));
        let rows = matches
            .iter()
            .take(self.limit)
            .map(|(index, event)| {
                (
                    index,
                    event.timestamp - fight.start_time,
                    &event.kind,
                    event
                        .ability_game_id
                        .filter(|id| *id >= 0)
                        .map(|id| format!("{id:X}")),
                    event.ability_game_id.and_then(|id| abilities.get(&id)),
                    event
                        .source_id
                        .and_then(|id| actors.get(&id))
                        .map(|actor| &actor.name),
                    event
                        .target_id
                        .and_then(|id| actors.get(&id))
                        .map(|actor| &actor.name),
                )
            })
            .collect::<Vec<_>>();
        let report = json!({
            "input": self.input.as_str(), "sha256": super::hash(&bytes), "report": log.report.code, "fight": fight.id,
            "origin_timestamp_ms": fight.start_time, "around_index": self.around_index, "window_timestamp_ms": window,
            "columns": ["raw_index", "at_ms", "type", "id", "name", "source", "target"],
            "total_matches": total_matches, "returned": rows.len(), "omitted": total_matches - rows.len(), "rows": rows,
            "signal_namespace": "raw FFLogs; no ACT projection"
        });
        println!("{}", serde_json::to_string(&report)?);
        Ok(())
    }
}

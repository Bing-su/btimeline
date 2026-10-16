//! Prefer complete, uninterrupted observations so wipe reactions do not define the normal fight.
//! For example, three clean clears can replace dozens of short progression pulls.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use super::multi::Input;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Selection {
    policy: &'static str,
    selected: Vec<String>,
    excluded: Vec<ExcludedInput>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExcludedInput {
    file: String,
    sha256: String,
    reason: &'static str,
    kill: bool,
    boss_cast_starts: usize,
}

fn starts(input: &Input) -> usize {
    input
        .pull
        .occurrences
        .iter()
        .filter(|row| row.kind == "begincast" && row.role.ends_with("/Boss"))
        .count()
}

fn retried(input: &Input) -> bool {
    input
        .report
        .get("bossAttempts")
        .and_then(Value::as_array)
        .is_some_and(|attempts| attempts.iter().any(|attempt| attempt["selected"] == false))
}

// Preserve a longer final attempt, e.g. slow clears can reveal a loop absent from three fast clears.
fn boss_trace(input: &Input) -> BTreeMap<i64, usize> {
    let mut counts = BTreeMap::new();
    for signal in input
        .signals
        .values()
        .filter(|signal| signal.key.kind == "cast" && signal.key.role.ends_with("/Boss"))
    {
        *counts.entry(signal.key.actor_game_id).or_default() += 1;
    }
    counts
}

pub(super) fn select(inputs: &mut Vec<Input>) -> Result<Selection> {
    let clean_clears = inputs
        .iter()
        .filter(|input| input.pull.kill && !retried(input))
        .count();
    let clears = inputs.iter().filter(|input| input.pull.kill).count();
    let mut clean_lengths = BTreeMap::new();
    for input in inputs
        .iter()
        .filter(|input| input.pull.kill && !retried(input))
    {
        for (boss, count) in boss_trace(input) {
            // Keep two corroborating traces, e.g. a single long clear must not discard its only peer.
            let lengths = clean_lengths.entry(boss).or_insert((0, 0));
            if count > lengths.0 {
                *lengths = (count, lengths.0);
            } else {
                lengths.1 = lengths.1.max(count);
            }
        }
    }
    let most_starts = inputs.iter().map(starts).max().unwrap_or(0);
    // Keep at least three substantial observations before pruning prefixes, e.g. one long wipe is not a consensus.
    let substantial = inputs
        .iter()
        .filter(|input| starts(input) * 4 >= most_starts)
        .count();
    let mut excluded = Vec::new();
    let mut selected = Vec::new();
    for input in inputs.drain(..) {
        let extends_boss = !input.pull.boss_spans.is_empty()
            && boss_trace(&input).iter().any(|(boss, count)| {
                *count > clean_lengths.get(boss).map_or(0, |lengths| lengths.1)
            });
        let reason = if clean_clears >= 3 && retried(&input) && !extends_boss {
            Some("retryTraceCoveredByCleanClears")
        } else if clears >= 3 && !input.pull.kill {
            Some("wipeCoveredByClears")
        } else if clears < 3
            && substantial >= 3
            && !input.pull.kill
            && starts(&input) * 4 < most_starts
        {
            Some("shortProgressionPrefix")
        } else {
            None
        };
        if let Some(reason) = reason {
            excluded.push(ExcludedInput {
                file: input.pull.file.clone(),
                sha256: input
                    .report
                    .pointer("/input/sha256")
                    .and_then(Value::as_str)
                    .context("Missing selected input hash")?
                    .to_owned(),
                reason,
                kill: input.pull.kill,
                boss_cast_starts: starts(&input),
            });
        } else {
            selected.push(input);
        }
    }
    *inputs = selected;
    Ok(Selection {
        policy: "Prefer at least three clears, then uninterrupted clears; retain longer final boss attempts to observe repeated suffixes. Otherwise exclude only wipes below one quarter of the longest boss-start trace when three substantial observations remain. Excluded inputs are replayed diagnostically, not used to infer mechanics.",
        selected: inputs.iter().map(|input| input.pull.file.clone()).collect(),
        excluded,
    })
}

pub(super) fn annotate(mut report: Value, selection: Selection) -> Result<Value> {
    report
        .as_object_mut()
        .context("Missing generation report object")?
        .insert("inputSelection".into(), serde_json::to_value(selection)?);
    Ok(report)
}

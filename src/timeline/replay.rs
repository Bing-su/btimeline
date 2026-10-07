use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{Destination, Entry, FieldPattern, JumpWhen, LogType, Sync};

// Preserve raw indices for the oracle, e.g. a friendly cast can match a boss sync accidentally.
pub(crate) struct Signal {
    pub index: usize,
    pub at_ms: i64,
    pub log: &'static str,
    pub fields: BTreeMap<String, String>,
}

#[derive(Default)]
pub(crate) struct Evidence {
    pub expected: BTreeMap<usize, BTreeSet<usize>>,
    pub censored: BTreeSet<usize>,
    pub missing: BTreeSet<usize>,
    // Preserve mandatory finite evidence, e.g. an exit jump cannot hide missing repeat rounds.
    pub required_missing: BTreeSet<usize>,
    pub inactive: BTreeSet<usize>,
    pub lookahead_ms: Option<i64>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    pub active_sync_rows: usize,
    pub disabled_sync_rows: usize,
    pub matches: usize,
    pub wrong_matches: usize,
    pub ambiguous_matches: usize,
    pub wrong_jumps: usize,
    pub dependency_violations: usize,
    pub window_misses: usize,
    pub missing: usize,
    pub censored: usize,
    pub unsupported: usize,
    pub unverified: usize,
    pub max_abs_error_ms: i64,
}

impl Summary {
    pub(crate) fn passed(&self) -> bool {
        self.wrong_matches
            + self.ambiguous_matches
            + self.wrong_jumps
            + self.dependency_violations
            + self.window_misses
            + self.missing
            + self.unsupported
            + self.unverified
            == 0
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    event_index: usize,
    source_ms: i64,
    clock_before_ms: i64,
    clock_after_ms: i64,
    error_ms: i64,
    expected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Row {
    pub entry_index: usize,
    pub at_ms: i64,
    pub name: String,
    pub status: &'static str,
    pub matches: Vec<Match>,
    pub observations: Vec<Observation>,
    pub expected_event_indices: BTreeSet<usize>,
    pub window_miss_event_indices: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Observation {
    event_index: usize,
    source_ms: i64,
    clock_ms: i64,
    error_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JumpTrace {
    entry_index: usize,
    source_ms: i64,
    from_ms: i64,
    to_ms: i64,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Replay {
    pub passed: bool,
    pub summary: Summary,
    pub rows: Vec<Row>,
    pub(crate) jumps: Vec<JumpTrace>,
    final_clock_ms: i64,
    reset_clock_ms: i64,
    pub(crate) previews: Vec<Preview>,
}

// This independent projection audits visible rows separately from sync activation, e.g. after a jump.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    source_ms: i64,
    clock_ms: i64,
    moment: &'static str,
    horizon_ms: i64,
    entry_indices: Vec<usize>,
}

struct Predicate {
    log: String,
    fields: BTreeMap<String, Vec<regress::Regex>>,
    window: [i64; 2],
    supported: bool,
    enabled: bool,
}

fn ms(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn predicate(sync: &Sync) -> Result<Predicate> {
    let mut fields = BTreeMap::new();
    let (log, supported) = match sync {
        Sync::Network(sync) => {
            let log = format!("{:?}", sync.log);
            let supported_fields: &[&str] = match sync.log {
                LogType::Ability | LogType::StartsUsing => &["id", "source", "name", "target"],
                _ => &[],
            };
            for (key, value) in &sync.fields {
                let patterns = match value {
                    FieldPattern::One(pattern) => vec![pattern.as_str()],
                    FieldPattern::Many(patterns) => patterns.iter().map(String::as_str).collect(),
                };
                fields.insert(
                    key.clone(),
                    patterns
                        .into_iter()
                        .map(|pattern| regress::Regex::with_flags(pattern, "i"))
                        .collect::<Result<_, _>>()?,
                );
            }
            let supported = !supported_fields.is_empty()
                && sync
                    .fields
                    .keys()
                    .all(|key| supported_fields.contains(&key.as_str()));
            (log, supported)
        }
        // FFLogs has no ACT raw lines; regex syncs must remain unverified rather than fabricated.
        Sync::Regex(_) => (String::new(), false),
    };
    Ok(Predicate {
        log,
        fields,
        supported,
        enabled: sync.enabled(),
        window: sync.window().unwrap_or([2.5, 2.5]).map(ms),
    })
}

impl Predicate {
    fn matches(&self, signal: &Signal) -> bool {
        self.supported
            && self.log == signal.log
            && self.fields.iter().all(|(key, patterns)| {
                signal
                    .fields
                    .get(key)
                    .is_some_and(|value| patterns.iter().any(|p| p.find(value).is_some()))
            })
    }
}

// The clock chooses syncs without consulting correspondence. Evidence only audits that choice afterward.
// Example: an excluded cast with the same ID/source produces wrongMatches, not an oracle-selected match.
pub(crate) fn run(
    source: &str,
    signals: &[Signal],
    end_ms: i64,
    evidence: &Evidence,
) -> Result<Replay> {
    let timeline = super::parse(source)?;
    let limit_ms = i64::MAX / 4;
    let valid_seconds = |seconds: f64| seconds <= limit_ms as f64 / 1000.0;
    ensure!(
        end_ms >= 0
            && end_ms <= limit_ms
            && signals.iter().all(|s| s.at_ms >= 0 && s.at_ms <= end_ms)
            && signals
                .windows(2)
                .all(|pair| matches!(pair, [a,b] if a.at_ms <= b.at_ms)),
        "Invalid replay signal order or time range"
    );
    for entry in &timeline.entries {
        match entry {
            Entry::Event { at, sync, jump, .. } => {
                ensure!(
                    valid_seconds(*at)
                        && sync
                            .as_ref()
                            .and_then(Sync::window)
                            .is_none_or(|w| w.into_iter().all(valid_seconds))
                        && jump.as_ref().is_none_or(|j| match j.to {
                            Destination::Time(at) => valid_seconds(at),
                            Destination::Label(_) => true,
                        }),
                    "Timeline time exceeds replay clock range"
                );
            }
            Entry::Label { at, .. } => {
                ensure!(valid_seconds(*at), "Label exceeds replay clock range")
            }
            _ => {}
        }
    }
    let labels: BTreeMap<_, _> = timeline
        .entries
        .iter()
        .filter_map(|entry| {
            if let Entry::Label { at, name } = entry {
                Some((name.as_str(), ms(*at)))
            } else {
                None
            }
        })
        .collect();
    let mut rows = Vec::new();
    let mut predicates = Vec::new();
    let mut destinations = Vec::new();
    let mut always = Vec::new();
    let mut summary = Summary::default();
    for (entry_index, entry) in timeline.entries.iter().enumerate() {
        let Entry::Event {
            at,
            name,
            sync,
            jump,
            ..
        } = entry
        else {
            continue;
        };
        let pred = sync.as_ref().map(predicate).transpose()?;
        let supported = pred.as_ref().is_some_and(|p| p.supported);
        let enabled = pred.as_ref().is_some_and(|p| p.enabled);
        let expected = evidence
            .expected
            .get(&entry_index)
            .cloned()
            .unwrap_or_default();
        if enabled && !supported {
            summary.unsupported += 1;
        }
        if enabled {
            summary.active_sync_rows += 1;
        } else {
            summary.disabled_sync_rows += 1;
        }
        rows.push(Row {
            entry_index,
            at_ms: ms(*at),
            name: name.clone(),
            status: if enabled && !supported {
                "unsupportedSync"
            } else {
                "pending"
            },
            matches: Vec::new(),
            observations: Vec::new(),
            expected_event_indices: expected,
            window_miss_event_indices: Vec::new(),
        });
        predicates.push(pred);
        destinations.push(
            jump.as_ref()
                .map(|jump| match &jump.to {
                    Destination::Time(at) => Ok(ms(*at)),
                    Destination::Label(name) => labels
                        .get(name.as_str())
                        .copied()
                        .context("Missing jump label"),
                })
                .transpose()?,
        );
        always.push(
            jump.as_ref()
                .is_some_and(|j| matches!(j.when, JumpWhen::Always)),
        );
    }
    let mut elapsed = 0;
    let mut clock = 0;
    let mut spent = BTreeSet::new();
    let mut jumps = Vec::new();
    let mut last_expected_entry = None;
    let mut stopped = false;
    let mut skipped = BTreeSet::new();
    let horizon_ms = evidence.lookahead_ms.unwrap_or(30_000);
    ensure!(
        (0..=3_600_000).contains(&horizon_ms),
        "Invalid preview horizon"
    );
    let mut previews = Vec::new();
    let preview = |rows: &[Row], source_ms, clock_ms, moment| Preview {
        source_ms,
        clock_ms,
        moment,
        horizon_ms,
        entry_indices: rows
            .iter()
            .filter(|row| {
                row.at_ms >= clock_ms
                    && row.at_ms <= clock_ms + horizon_ms
                    && !timeline.hide_names.contains(&row.name)
            })
            .map(|row| row.entry_index)
            .collect(),
    };
    // Include a final tick so a scheduled forcejump cannot disappear just because no cast follows it.
    for signal in signals.iter().map(Some).chain(std::iter::once(None)) {
        let at = signal.map_or(end_ms, |s| s.at_ms);
        let mut remaining = at - elapsed;
        loop {
            let scheduled = rows
                .iter()
                .enumerate()
                .filter(|(i, row)| {
                    always.get(*i) == Some(&true)
                        && !spent.contains(i)
                        && row.at_ms >= clock
                        && row.at_ms <= clock + remaining
                })
                .min_by_key(|(i, row)| (row.at_ms, *i))
                .map(|(i, row)| (i, row.at_ms));
            let Some((i, from)) = scheduled else { break };
            if rows
                .iter()
                .enumerate()
                .filter(|(j, row)| {
                    always.get(*j) == Some(&true) && !spent.contains(j) && row.at_ms == from
                })
                .count()
                > 1
            {
                summary.unsupported += 1;
                stopped = true;
                break;
            }
            if from == clock + remaining
                && signal.is_some_and(|signal| {
                    predicates
                        .iter()
                        .zip(&rows)
                        .enumerate()
                        .any(|(j, (pred, row))| {
                            !spent.contains(&j)
                                && pred.as_ref().is_some_and(|p| {
                                    p.enabled
                                        && p.matches(signal)
                                        && from >= row.at_ms - p.window[0]
                                        && from <= row.at_ms + p.window[1]
                                })
                        })
                })
            {
                if predicates
                    .get(i)
                    .and_then(Option::as_ref)
                    .is_some_and(|p| signal.is_some_and(|s| p.matches(s)))
                    && !predicates
                        .iter()
                        .zip(&rows)
                        .enumerate()
                        .any(|(j, (pred, row))| {
                            j != i
                                && !spent.contains(&j)
                                && pred.as_ref().is_some_and(|p| {
                                    p.enabled
                                        && signal.is_some_and(|s| p.matches(s))
                                        && from >= row.at_ms - p.window[0]
                                        && from <= row.at_ms + p.window[1]
                                })
                        })
                {
                    break;
                }
                // Exit sync vs forcejump priority is not specified by P0; never claim compatibility here.
                summary.unsupported += 1;
                stopped = true;
                break;
            }
            let to = destinations
                .get(i)
                .copied()
                .flatten()
                .context("Missing forcejump destination")?;
            // ponytail: cap at 10,000 jumps per pull; add cycle analysis if valid longer replays need it.
            if jumps.len() >= 10_000 || to == from {
                summary.wrong_jumps += 1;
                stopped = true;
                break;
            }
            skipped.extend(
                rows.iter()
                    .enumerate()
                    .filter(|(_, row)| row.at_ms > from && row.at_ms < to)
                    .map(|(i, _)| i),
            );
            remaining -= from - clock;
            jumps.push(JumpTrace {
                entry_index: rows.get(i).context("Missing jump row")?.entry_index,
                source_ms: at - remaining,
                from_ms: from,
                to_ms: to,
                reason: "always",
            });
            clock = to;
            last_expected_entry = None;
            spent.clear();
            if to > from {
                spent.insert(i);
            }
        }
        if stopped {
            break;
        }
        clock += remaining;
        elapsed = at;
        let Some(signal) = signal else { break };
        let mut candidates: Vec<_> = predicates
            .iter()
            .zip(&rows)
            .enumerate()
            .filter(|(i, (pred, row))| {
                !spent.contains(i)
                    && pred.as_ref().is_some_and(|p| {
                        p.enabled
                            && p.matches(signal)
                            && clock >= row.at_ms - p.window[0]
                            && clock <= row.at_ms + p.window[1]
                    })
            })
            .map(|(i, _)| i)
            .collect();
        // Do not choose a branch by raw tie order, e.g. X and Y complete at the same millisecond.
        // Check simultaneous signals before a jump moves the clock out of its sibling's window.
        if candidates
            .iter()
            .any(|i| destinations.get(*i).is_some_and(Option::is_some))
        {
            let concurrent: BTreeSet<_> = signals
                .iter()
                .filter(|s| s.at_ms == signal.at_ms)
                .flat_map(|s| {
                    predicates
                        .iter()
                        .zip(&rows)
                        .enumerate()
                        .filter(|(i, (pred, row))| {
                            !spent.contains(i)
                                && destinations.get(*i).is_some_and(Option::is_some)
                                && pred.as_ref().is_some_and(|p| {
                                    p.enabled
                                        && p.matches(s)
                                        && clock >= row.at_ms - p.window[0]
                                        && clock <= row.at_ms + p.window[1]
                                })
                        })
                        .map(|(i, _)| i)
                })
                .collect();
            if concurrent.len() > 1 {
                summary.ambiguous_matches += 1;
                candidates.clear();
            }
        }
        if candidates.len() > 1 {
            summary.ambiguous_matches += 1;
        }
        // Audit intended signals separately, including observations outside an active window.
        for (i, row) in rows.iter_mut().enumerate() {
            if row.expected_event_indices.contains(&signal.index) {
                let error_ms = clock - row.at_ms;
                summary.max_abs_error_ms = summary.max_abs_error_ms.max(error_ms.abs());
                row.observations.push(Observation {
                    event_index: signal.index,
                    source_ms: signal.at_ms,
                    clock_ms: clock,
                    error_ms,
                });
                if predicates
                    .get(i)
                    .and_then(Option::as_ref)
                    .is_some_and(|p| p.supported && !p.matches(signal))
                {
                    summary.dependency_violations += 1;
                }
            }
            if row.expected_event_indices.contains(&signal.index)
                && predicates
                    .get(i)
                    .and_then(Option::as_ref)
                    .is_some_and(|p| p.enabled && p.supported)
                && !candidates.contains(&i)
            {
                row.window_miss_event_indices.push(signal.index);
                summary.window_misses += 1;
            }
        }
        let [i] = candidates.as_slice() else { continue };
        let row = rows.get_mut(*i).context("Missing sync row")?;
        let expected = row.expected_event_indices.contains(&signal.index);
        if !expected
            && (!row.expected_event_indices.is_empty()
                || evidence.censored.contains(&row.entry_index)
                || evidence.missing.contains(&row.entry_index)
                || evidence.inactive.contains(&row.entry_index))
        {
            summary.wrong_matches += 1;
        }
        if expected && last_expected_entry.is_some_and(|last| *i < last) {
            summary.dependency_violations += 1;
        }
        if expected {
            last_expected_entry = Some(*i);
        }
        let error = clock - row.at_ms;
        summary.max_abs_error_ms = summary.max_abs_error_ms.max(error.abs());
        let to = destinations.get(*i).copied().flatten().unwrap_or(row.at_ms);
        if destinations.get(*i).is_some_and(Option::is_some) {
            if !expected {
                summary.wrong_jumps += 1;
            }
            jumps.push(JumpTrace {
                entry_index: row.entry_index,
                source_ms: signal.at_ms,
                from_ms: clock,
                to_ms: to,
                reason: "sync",
            });
            spent.clear();
            last_expected_entry = None;
        }
        row.matches.push(Match {
            event_index: signal.index,
            source_ms: signal.at_ms,
            clock_before_ms: clock,
            clock_after_ms: to,
            error_ms: error,
            expected,
        });
        summary.matches += 1;
        let from = row.at_ms;
        if destinations.get(*i).is_some_and(Option::is_some) {
            previews.push(preview(&rows, signal.at_ms, clock, "beforeJump"));
            previews.push(preview(&rows, signal.at_ms, to, "afterJump"));
        }
        skipped.extend(
            rows.iter()
                .enumerate()
                .filter(|(_, row)| row.at_ms > from && row.at_ms < to)
                .map(|(i, _)| i),
        );
        clock = to;
        if destinations.get(*i).is_none_or(Option::is_none)
            || predicates
                .get(*i)
                .and_then(Option::as_ref)
                .is_some_and(|p| to >= from - p.window[0])
        {
            spent.insert(*i);
        }
        // A sync jump to zero stops this replay, e.g. a wipe reset; the next file starts at zero independently.
        if to == 0 && destinations.get(*i).is_some_and(Option::is_some) {
            break;
        }
    }
    for (i, row) in rows.iter_mut().enumerate() {
        if skipped.contains(&i) && !row.expected_event_indices.is_empty() && row.matches.is_empty()
        {
            summary.wrong_jumps += 1;
            summary.dependency_violations += 1;
        }
        // A historical skip cannot hide later activation, e.g. a backward jump revisits an unresolved row.
        if row.expected_event_indices.is_empty()
            && !evidence.censored.contains(&row.entry_index)
            && !evidence.missing.contains(&row.entry_index)
            && !evidence.inactive.contains(&row.entry_index)
            && (!skipped.contains(&i) || !row.matches.is_empty())
        {
            summary.unverified += 1;
        }
        if row.status == "unsupportedSync" {
            continue;
        }
        row.status = if evidence.required_missing.contains(&row.entry_index) {
            summary.missing += 1;
            "missingExpectedSignal"
        } else if !row.matches.is_empty() {
            if row.matches.iter().all(|m| m.expected) {
                "matched"
            } else {
                "unexpectedMatch"
            }
        } else if !row.window_miss_event_indices.is_empty() {
            "windowMiss"
        } else if row.expected_event_indices.is_empty()
            && (skipped.contains(&i) || evidence.inactive.contains(&row.entry_index))
        {
            // A branch can intentionally omit an oracle gap, e.g. A jumps over B to the observed C.
            "notVisited"
        } else if evidence.censored.contains(&row.entry_index) {
            summary.censored += 1;
            "unobservedAfterEnd"
        } else if evidence.missing.contains(&row.entry_index) {
            summary.missing += 1;
            "missingExpectedSignal"
        } else if row.expected_event_indices.is_empty() {
            "unverified"
        } else if predicates
            .get(i)
            .and_then(Option::as_ref)
            .is_none_or(|p| !p.enabled)
        {
            if row.observations.len() == row.expected_event_indices.len() {
                "observedWithoutSync"
            } else {
                summary.missing += 1;
                "missing"
            }
        } else if skipped.contains(&i) {
            summary.dependency_violations += 1;
            "unexpectedlySkipped"
        } else {
            summary.missing += 1;
            "missing"
        };
    }
    Ok(Replay {
        passed: summary.passed(),
        summary,
        rows,
        jumps,
        final_clock_ms: clock,
        reset_clock_ms: 0,
        previews,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn signal(index: usize, at_ms: i64, id: &str) -> Signal {
        Signal {
            index,
            at_ms,
            log: "Ability",
            fields: BTreeMap::from([
                ("id".into(), id.into()),
                ("source".into(), "New Boss".into()),
            ]),
        }
    }

    fn event(at: f64, id: &str) -> serde_json::Value {
        json!({"kind":"event", "at":at, "name":id,
            "sync":{"log":"Ability", "fields":{"id":format!("^{id}$"),"source":"^New Boss$"}}})
    }

    fn play(
        entries: Vec<serde_json::Value>,
        signals: &[Signal],
        expected: &[(usize, &[usize])],
        censored: &[usize],
        end_ms: i64,
    ) -> Replay {
        let evidence = Evidence {
            expected: expected
                .iter()
                .map(|(i, indices)| (*i, indices.iter().copied().collect()))
                .collect(),
            censored: censored.iter().copied().collect(),
            missing: BTreeSet::new(),
            ..Default::default()
        };
        run(
            &json!({"schemaVersion":1,"entries":entries}).to_string(),
            signals,
            end_ms,
            &evidence,
        )
        .expect("valid fixture")
    }

    #[rstest::rstest]
    #[case(7500, true)]
    #[case(12500, true)]
    #[case(7499, false)]
    #[case(12501, false)]
    fn inclusive_window_and_clock_error(#[case] at: i64, #[case] passed: bool) {
        let result = play(
            vec![event(10.0, "B528")],
            &[signal(0, at, "B528")],
            &[(0, &[0])],
            &[],
            15000,
        );
        assert_eq!(result.passed, passed);
        assert_eq!(result.summary.window_misses, usize::from(!passed));
        assert_eq!(result.summary.max_abs_error_ms, (at - 10000).abs());
    }

    #[test]
    fn repeated_ids_and_reset_start_independently() {
        let signals = [
            signal(0, 1100, "B528"),
            signal(1, 5100, "B528"),
            signal(2, 9100, "B528"),
        ];
        let entries = vec![event(1.0, "B528"), event(5.0, "B528"), event(9.0, "B528")];
        let first = play(
            entries.clone(),
            &signals,
            &[(0, &[0]), (1, &[1]), (2, &[2])],
            &[],
            10000,
        );
        let second = play(
            entries,
            &signals,
            &[(0, &[0]), (1, &[1]), (2, &[2])],
            &[],
            10000,
        );
        assert!(first.passed && second.passed);
        assert_eq!(first.summary.matches, 3);
        assert_eq!(
            serde_json::to_value(first).expect("serialize"),
            serde_json::to_value(second).expect("serialize")
        );
    }

    #[test]
    fn overlap_and_excluded_cast_do_not_use_oracle_to_choose() {
        let result = play(
            vec![event(1.0, "A"), event(2.0, "A")],
            &[signal(0, 1000, "A")],
            &[(0, &[0]), (1, &[1])],
            &[],
            5000,
        );
        assert_eq!(result.summary.ambiguous_matches, 1);
        assert_eq!(result.summary.matches, 0);
        let wrong = play(
            vec![event(1.0, "A")],
            &[signal(99, 900, "A"), signal(0, 1000, "A")],
            &[(0, &[0])],
            &[],
            5000,
        );
        assert_eq!(wrong.summary.wrong_matches, 1);
        assert_eq!(wrong.summary.window_misses, 1);
        assert!(!wrong.passed);
    }

    #[test]
    fn branch_entry_and_merge_skip_only_unobserved_path() {
        let mut opening = event(1.0, "A");
        opening["jump"] = json!({"to":"branch","when":"sync"});
        let entries = vec![
            opening,
            event(5.0, "OTHER"),
            json!({"kind":"label","at":20.0,"name":"branch"}),
            event(21.0, "B"),
            event(30.0, "MERGE"),
        ];
        let result = play(
            entries,
            &[
                signal(0, 1000, "A"),
                signal(1, 2000, "B"),
                signal(2, 11000, "MERGE"),
            ],
            &[(0, &[0]), (3, &[1]), (4, &[2])],
            &[],
            12000,
        );
        assert!(result.passed);
        assert_eq!(result.rows[1].status, "notVisited");
        assert_eq!(result.jumps[0].to_ms, 20000);
    }

    #[test]
    fn incorrect_jump_exposes_skipped_dependency() {
        let mut opening = event(1.0, "A");
        opening["jump"] = json!({"to":50.0,"when":"sync"});
        let result = play(
            vec![opening, event(20.0, "B")],
            &[signal(0, 1000, "A"), signal(1, 2000, "B")],
            &[(0, &[0]), (1, &[1])],
            &[],
            3000,
        );
        assert!(!result.passed);
        assert_eq!(result.summary.wrong_jumps, 1);
        assert_eq!(result.summary.dependency_violations, 1);
    }

    #[test]
    fn backward_jump_cannot_hide_activation_of_an_unverified_row() {
        let mut opening = event(1.0, "A");
        opening["jump"] = json!({"to":20.0,"when":"sync"});
        let mut repeat = event(21.0, "C");
        repeat["jump"] = json!({"to":1.0,"when":"sync"});
        let result = play(
            vec![opening, event(5.0, "B"), repeat],
            &[
                signal(0, 1000, "A"),
                signal(1, 2000, "C"),
                signal(2, 6000, "B"),
            ],
            &[(0, &[0]), (2, &[1])],
            &[],
            7000,
        );
        assert!(!result.passed);
        assert_eq!(result.summary.unverified, 1);
        assert_eq!(result.rows[1].status, "unexpectedMatch");
        assert_eq!(result.rows[1].matches.len(), 1);
        assert!(!result.rows[1].matches[0].expected);
    }

    #[test]
    fn backward_jump_replays_repeated_row_and_zero_jump_stops() {
        let mut repeat = event(10.0, "B528");
        repeat["jump"] = json!({"to":1.0,"when":"sync"});
        let result = play(
            vec![repeat],
            &[signal(0, 10000, "B528"), signal(1, 19000, "B528")],
            &[(0, &[0, 1])],
            &[],
            20000,
        );
        assert!(result.passed);
        assert_eq!(result.jumps.len(), 2);
        let mut reset = event(1.0, "EXIT");
        reset["jump"] = json!({"to":0,"when":"sync"});
        let result = play(
            vec![reset],
            &[signal(0, 1000, "EXIT"), signal(1, 1100, "EXIT")],
            &[(0, &[0])],
            &[],
            2000,
        );
        assert!(result.passed);
        assert_eq!(result.final_clock_ms, 0);
        assert_eq!(result.summary.matches, 1);
    }

    #[test]
    fn forcejump_runs_without_casts_but_priority_and_nonadvancing_cycle_fail() {
        let mut force = event(1.0, "EXIT");
        force["jump"] = json!({"to":2.0,"when":"always"});
        let result = play(vec![force.clone()], &[], &[], &[0], 1000);
        assert_eq!(result.jumps.len(), 1);
        assert_eq!(result.final_clock_ms, 2000);
        let collision = play(
            vec![force.clone(), event(1.0, "OTHER_EXIT")],
            &[signal(0, 1000, "OTHER_EXIT")],
            &[(1, &[0])],
            &[0],
            1000,
        );
        assert!(!collision.passed);
        assert_eq!(collision.summary.unsupported, 1);
        let own = play(
            vec![force.clone()],
            &[signal(0, 1000, "EXIT")],
            &[(0, &[0])],
            &[],
            1000,
        );
        assert!(own.passed);
        assert_eq!(own.summary.unsupported, 0);
        force["jump"]["to"] = json!(1.0);
        let cycle = play(vec![force], &[], &[], &[0], 1000);
        assert!(!cycle.passed);
        assert_eq!(cycle.summary.wrong_jumps, 1);
    }

    #[test]
    fn termination_missing_unsupported_and_disabled_sync_are_distinct() {
        let result = play(
            vec![event(1.0, "A"), event(10.0, "B")],
            &[],
            &[(0, &[0])],
            &[1],
            2000,
        );
        assert_eq!(result.summary.missing, 1);
        assert_eq!(result.summary.censored, 1);
        let mut disabled = event(1.0, "A");
        disabled["sync"]["enabled"] = json!(false);
        let result = play(
            vec![disabled],
            &[signal(0, 1100, "A")],
            &[(0, &[0])],
            &[],
            2000,
        );
        assert!(result.passed);
        assert_eq!(result.rows[0].status, "observedWithoutSync");
        assert_eq!(result.summary.matches, 0);
        assert_eq!(result.summary.max_abs_error_ms, 100);
        let result = play(
            vec![json!({"kind":"event","at":1,"name":"raw","sync":{"regex":"foo"}})],
            &[],
            &[],
            &[],
            2000,
        );
        assert!(!result.passed);
        assert_eq!(result.summary.unsupported, 1);
        assert_eq!(result.summary.unverified, 1);
    }
}

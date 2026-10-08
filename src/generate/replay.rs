use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use path_slash::PathBufExt as _;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{GroupKey, Pull, input, load_one, multi};
use crate::timeline::replay::{self, Evidence, Signal};

// Read only the evidence fields needed by replay, e.g. timing statistics do not select source rows.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvidenceSample {
    file: String,
    event_indices: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayPull<'a> {
    file: &'a str,
    sha256: String,
    report: &'a str,
    fight: i64,
    revision: i64,
    log_version: i64,
    end_ms: i64,
    termination: &'static str,
    unrepresented_event_indices: Vec<usize>,
    unfinished_starts: Vec<usize>,
    replay: replay::Replay,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayValidation {
    #[serde(flatten)]
    validation: super::report::Validation,
    replay_executed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayPolicy {
    clock: &'static str,
    window: &'static str,
    ambiguity: &'static str,
    supported_signals: [&'static str; 2],
    coverage: &'static str,
    unsupported: &'static str,
    lookahead: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayReport<'a> {
    status: &'static str,
    tool_version: &'static str,
    timeline: String,
    timeline_sha256: String,
    evidence_sha256: String,
    evidence: String,
    group: &'a super::GroupKey,
    pulls: &'a [ReplayPull<'a>],
    validation: ReplayValidation,
    policy: ReplayPolicy,
}

pub(super) fn signals(log: &crate::fflogs::model::CollectedLog) -> Result<Vec<Signal>> {
    let master = &log.report.master_data;
    let actors: BTreeMap<_, _> = master.actors.iter().map(|a| (a.id, &a.name)).collect();
    let abilities: BTreeMap<_, _> = master
        .abilities
        .iter()
        .map(|a| (a.game_id, &a.name))
        .collect();
    let start = log
        .report
        .fights
        .first()
        .context("Missing fight")?
        .start_time;
    let mut result = Vec::new();
    // Replay all raw casts, including excluded/friendly/melee events, to expose accidental activation.
    for (index, event) in log.events.iter().enumerate() {
        let log = match event.kind.as_str() {
            "cast" => "Ability",
            "begincast" => "StartsUsing",
            _ => continue,
        };
        let mut fields = BTreeMap::new();
        if let Some(id) = event.ability_game_id {
            fields.insert("id".into(), format!("{id:X}"));
            if let Some(name) = abilities.get(&id) {
                fields.insert("name".into(), (*name).clone());
            }
        }
        for (key, id) in [("source", event.source_id), ("target", event.target_id)] {
            if let Some(name) = id.and_then(|id| actors.get(&id)) {
                fields.insert(key.into(), (*name).clone());
            }
        }
        result.push(Signal {
            index,
            at_ms: event.timestamp - start,
            log,
            fields,
        });
    }
    result.sort_by_key(|signal| signal.at_ms);
    Ok(result)
}

fn same_file(file: &str, input: &str) -> bool {
    fs::canonicalize(PathBuf::from_slash(file))
        .ok()
        .zip(fs::canonicalize(PathBuf::from_slash(input)).ok())
        .is_some_and(|(a, b)| a == b)
}

fn after_end(
    peer: &Pull,
    target: &Pull,
    index: usize,
    relation: &multi::Correspondence,
) -> Result<bool> {
    let sample = peer
        .occurrences
        .iter()
        .find(|row| row.event_index == index)
        .context("Missing timing sample")?;
    // A reached successor proves an interior gap, e.g. A,B,C versus A,C is not a truncated B.
    if peer.occurrences.iter().any(|row| {
        (row.relative_ms, row.event_index) > (sample.relative_ms, sample.event_index)
            && relation.matched.contains_key(&row.event_index)
    }) {
        return Ok(false);
    }
    let anchor = peer
        .occurrences
        .iter()
        .filter(|row| {
            row.kind == "cast"
                && row.role.ends_with("/Boss")
                && row.relative_ms <= sample.relative_ms
        })
        .filter_map(|row| {
            relation
                .matched
                .get(&row.event_index)
                .and_then(|index| {
                    target
                        .occurrences
                        .iter()
                        .find(|target| target.event_index == *index)
                })
                .map(|target| (row.relative_ms, target.relative_ms))
        })
        .max_by_key(|(at, _)| *at)
        .unwrap_or((0, 0));
    // Kill also truncates unobserved completions, e.g. the boss dies at 6s during a cast due at 9s.
    Ok(
        i128::from(sample.relative_ms) - i128::from(anchor.0) + i128::from(anchor.1)
            > i128::from(target.end_ms),
    )
}

// P5 correspondence is an evaluation oracle only; it never decides which sync the clock activates.
// Holdout mappings must agree across all reached training paths, e.g. repeated IDs cannot pick a convenient row.
pub(super) fn evidence(
    yaml: &str,
    report: &Value,
    key: &GroupKey,
    pull: &Pull,
    log: &crate::fflogs::model::CollectedLog,
    peers: &BTreeMap<&str, &Pull>,
) -> Result<Evidence> {
    if report.pointer("/repeats/accepted").and_then(Value::as_bool) == Some(true) {
        return super::repeat::fold_evidence(yaml, report, key, pull, log, peers);
    }
    let timeline: crate::timeline::Timeline = serde_saphyr::from_str(yaml)?;
    let entries: Vec<_> = timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            matches!(entry, crate::timeline::Entry::Event { .. }) && !entry.is_combat_start()
        })
        .map(|(i, _)| i)
        .collect();
    let slots = report["slots"]
        .as_array()
        .context("Missing generation slots")?;
    ensure!(
        entries.len() == slots.len(),
        "Replay evidence requires the generated event rows in their original order"
    );
    ensure!(
        report["group"] == serde_json::to_value(key)?,
        "Replay input group differs from generation group"
    );
    let single = report.get("input").is_some();
    // Resolve path aliases once per pull, e.g. relative and absolute names still share exact evidence.
    let known_files: BTreeSet<_> = peers
        .keys()
        .copied()
        .filter(|file| same_file(file, &pull.file))
        .collect();
    let mut relations = BTreeMap::new();
    let mut result = Evidence::default();
    for (&entry, slot) in entries.iter().zip(slots) {
        let samples = if single {
            vec![EvidenceSample {
                file: report
                    .pointer("/input/file")
                    .and_then(Value::as_str)
                    .context("Missing generation file")?
                    .to_owned(),
                event_indices: vec![
                    serde_json::from_value(slot["eventIndex"].clone())
                        .context("Missing sample event index")?,
                ],
            }]
        } else {
            serde_json::from_value::<Vec<EvidenceSample>>(slot["samples"].clone())
                .context("Missing slot samples")?
        };
        let mut outcomes = Vec::new();
        for sample in &samples {
            let file = sample.file.as_str();
            let index = *sample
                .event_indices
                .first()
                .context("Missing sample event index")?;
            if !relations.contains_key(file) {
                let peer = peers
                    .get(file)
                    .copied()
                    .context("Sample file absent from generation inputs")?;
                if !known_files.contains(file) {
                    relations.insert(file.to_owned(), multi::correspondence(peer, pull)?.0);
                }
            }
            let peer = peers.get(file).copied().context("Missing evidence peer")?;
            let known = known_files.contains(file);
            let relation = relations.get(file);
            let matched = if known {
                Some(index)
            } else {
                relation.and_then(|r| r.matched.get(&index)).copied()
            };
            let censored = !known
                && matched.is_none()
                && match relation {
                    Some(r) => r.censored.contains(&index) || after_end(peer, pull, index, r)?,
                    None => false,
                };
            ensure!(
                peer.occurrences
                    .iter()
                    .any(|row| row.event_index == index && row.kind == "cast"),
                "Evidence refers to a non-cast occurrence"
            );
            outcomes.push((matched, censored));
        }
        // Known training samples are exact evidence; other peers do not override their source index.
        let direct = samples
            .iter()
            .find(|sample| known_files.contains(sample.file.as_str()));
        if let Some(sample) = direct {
            let index = *sample
                .event_indices
                .first()
                .context("Missing direct index")?;
            result.expected.insert(entry, BTreeSet::from([index]));
        } else {
            let matches: BTreeSet<_> = outcomes.iter().filter_map(|(index, _)| *index).collect();
            if matches.len() == 1
                && outcomes
                    .iter()
                    .all(|(index, censored)| index.is_some() || *censored)
            {
                result.expected.insert(entry, matches);
            } else if !outcomes.is_empty() && outcomes.iter().all(|(_, censored)| *censored) {
                result.censored.insert(entry);
            } else if !outcomes.is_empty()
                && outcomes
                    .iter()
                    .all(|(index, censored)| index.is_none() && !censored)
            {
                result.missing.insert(entry);
            }
        }
    }
    // An oracle-selected path only audits activation; absent siblings must still fail if they match.
    // Example: the shared merge cast belongs to the chosen X→A path, not also to Y→B.
    if let Some(branches) = report
        .pointer("/extensions/branches")
        .and_then(Value::as_array)
    {
        for branch in branches {
            let paths = branch["paths"].as_array().context("Missing branch paths")?;
            let selected: Vec<_> = paths
                .iter()
                .filter(|path| {
                    path["selectorSlot"]
                        .as_u64()
                        .and_then(|i| entries.get(i as usize))
                        .is_some_and(|entry| result.expected.contains_key(entry))
                })
                .collect();
            if let [chosen] = selected.as_slice() {
                for path in paths.iter().filter(|path| *path != *chosen) {
                    for slot in path["slots"].as_array().context("Missing path slots")? {
                        let entry = *entries
                            .get(slot.as_u64().context("Invalid path slot")? as usize)
                            .context("Missing path entry")?;
                        result.expected.remove(&entry);
                        result.censored.remove(&entry);
                        result.missing.remove(&entry);
                        result.inactive.insert(entry);
                    }
                }
            }
        }
    }
    result.lookahead_ms = report
        .pointer("/extensions/lookaheadMs")
        .and_then(Value::as_i64);
    Ok(result)
}

fn load_peers(report: &Value) -> Result<BTreeMap<String, (Pull, String)>> {
    let inputs = if report.get("input").is_some() {
        vec![report]
    } else {
        report["inputs"]
            .as_array()
            .context("Missing generation inputs")?
            .iter()
            .collect()
    };
    // Validate peer bytes concurrently, then retain input-order errors and duplicate-key behavior.
    let loaded = inputs
        .into_par_iter()
        .map(|item| {
            let input = &item["input"];
            let file = input["file"]
                .as_str()
                .context("Missing generation input file")?;
            let source = load_one(&PathBuf::from_slash(file))?;
            if let Some(hash) = input.get("sha256") {
                ensure!(
                    hash == &source.sha256,
                    "Generation input bytes changed: {file}"
                );
            }
            let pull = source.pull;
            ensure!(
                report["group"] == serde_json::to_value(&source.key)?
                    && input["report"] == pull.report
                    && input["fight"] == pull.fight
                    && input["revision"] == pull.revision
                    && input["logVersion"] == pull.log_version
                    && input["gameVersion"] == pull.game_version
                    && item["endMs"] == pull.end_ms
                    && item["kill"] == pull.kill
                    && item["occurrences"] == serde_json::to_value(&pull.occurrences)?,
                "Generation evidence changed or crosses groups: {file}"
            );
            let mut pull = pull;
            if matches!(
                report.get("mode").and_then(Value::as_str),
                Some("dungeon" | "alliance")
            ) {
                input::filter_boss_spans(&mut pull, item)?;
            }
            Ok::<_, anyhow::Error>((file.to_owned(), (pull, source.sha256)))
        })
        .collect::<Vec<_>>();
    loaded.into_iter().collect()
}

pub(crate) fn replay_file(
    timeline: impl AsRef<Path>,
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    report_path: Option<&Path>,
) -> Result<()> {
    let timeline = timeline.as_ref();
    let output = output.as_ref();
    let markdown_path = output.with_extension("md");
    ensure!(
        !output.exists() && !markdown_path.exists(),
        "Output already exists"
    );
    let yaml = fs::read_to_string(timeline)?;
    crate::timeline::convert(&yaml)?;
    let report_path = report_path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| timeline.with_extension("report.json"));
    let report_bytes = fs::read(&report_path)
        .with_context(|| format!("Reading replay evidence {}", report_path.display()))?;
    let report: Value = serde_json::from_slice(&report_bytes)?;
    let group = input::select_group(
        input.as_ref(),
        None,
        Some(
            report
                .pointer("/group/encounter")
                .and_then(Value::as_i64)
                .context("Missing generation encounter")?,
        ),
        Some(
            report
                .pointer("/group/difficulty")
                .and_then(Value::as_i64)
                .context("Missing generation difficulty")?,
        ),
    )?;
    let peers = load_peers(&report)?;
    let peer_pulls = peers
        .iter()
        .map(|(file, (pull, _))| (file.as_str(), pull))
        .collect();
    // Each pull owns its clock; indexed collection preserves report order despite worker completion order.
    let pulls = group
        .pulls
        .par_iter()
        .map(|pull| {
            let mut source = load_one(&PathBuf::from_slash(&pull.file))?;
            if let Some((_, hash)) = peers
                .values()
                .find(|(peer, _)| same_file(&peer.file, &source.pull.file))
            {
                ensure!(
                    hash == &source.sha256,
                    "Replay input changed after evidence inspection"
                );
            }
            let signals = signals(&source.log)?;
            if matches!(
                report.get("mode").and_then(Value::as_str),
                Some("dungeon" | "alliance")
            ) {
                let draft = super::draft::build_single(&source, super::GenerateMode::Dungeon)?;
                input::filter_boss_spans(&mut source.pull, &draft.report)?;
            }
            let evidence = evidence(
                &yaml,
                &report,
                &source.key,
                &source.pull,
                &source.log,
                &peer_pulls,
            )?;
            let represented: BTreeSet<_> = evidence.expected.values().flatten().copied().collect();
            let replay = replay::run(&yaml, &signals, source.pull.end_ms, &evidence)?;
            Ok::<_, anyhow::Error>(ReplayPull {
                file: &pull.file,
                sha256: source.sha256,
                report: &pull.report,
                fight: pull.fight,
                revision: pull.revision,
                log_version: pull.log_version,
                end_ms: pull.end_ms,
                termination: if pull.kill { "kill" } else { "wipe" },
                unrepresented_event_indices: source
                    .pull
                    .occurrences
                    .iter()
                    .filter(|row| row.kind == "cast" && !represented.contains(&row.event_index))
                    .map(|row| row.event_index)
                    .collect(),
                unfinished_starts: pull
                    .occurrences
                    .iter()
                    .filter(|row| row.kind == "begincast" && row.completion_event_index.is_none())
                    .map(|row| row.event_index)
                    .collect(),
                replay,
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let passed = pulls.iter().all(|pull| pull.replay.passed);
    let result = ReplayReport {
        status: if passed {
            "internally_validated"
        } else {
            "draft"
        },
        tool_version: env!("CARGO_PKG_VERSION"),
        timeline: super::slash_path(timeline),
        timeline_sha256: super::sha256(yaml.as_bytes()),
        evidence_sha256: super::sha256(&report_bytes),
        evidence: super::slash_path(&report_path),
        group: &group.key,
        pulls: &pulls,
        validation: ReplayValidation {
            validation: super::report::Validation {
                replay: passed,
                ..Default::default()
            },
            replay_executed: true,
        },
        policy: ReplayPolicy {
            clock: "fight start assumes the declared InCombat entry signal; raw cast replay starts independently at zero and audits unique sync corrections",
            window: "lower boundary inclusive, upper boundary exclusive; absent window is +/-2500 ms",
            ambiguity: "multiple active matches are reported, never resolved by entry order",
            supported_signals: ["Ability", "StartsUsing"],
            coverage: "Only represented rows are validated; disabled syncs and unrepresented casts are reported separately.",
            unsupported: "ACT raw regex, unavailable network fields and simultaneous exit/forcejump priority remain unverified",
            lookahead: "independent projection before/after sync jumps; actual runtime display not executed",
        },
    };
    let mut markdown = String::from(
        "# 재생 결과\n\n| 원본 | 종료 | 통과 | 오매칭 | 모호 | jump 오류 | 의존성 위반 | window 미검출 | 누락 | 종료 잘림 | 미지원 | 미검증 | 최대 시간 오차 (ms) |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for pull in &pulls {
        let s = &pull.replay.summary;
        writeln!(
            markdown,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            pull.file.replace('|', "\\|").replace(['\r', '\n'], " "),
            pull.termination,
            pull.replay.passed,
            s.wrong_matches,
            s.ambiguous_matches,
            s.wrong_jumps,
            s.dependency_violations,
            s.window_misses,
            s.missing,
            s.censored,
            s.unsupported,
            s.unverified,
            s.max_abs_error_ms
        )?;
    }
    markdown.push_str("\n행별 대응·시각·종료 잘림은 JSON의 `pulls[].replay.rows`에 기록합니다. cactbot parser/runtime과 lookahead 표시는 미실행입니다.\n");
    let bytes = format!("{}\n", serde_json::to_string_pretty(&result)?);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::output::write_new(&[
        (output, bytes.as_bytes()),
        (&markdown_path, markdown.as_bytes()),
    ])?;
    // Publish failure evidence before returning an error so CLI/holdout jobs can retain diagnostics.
    ensure!(passed, "Replay did not pass; see {}", output.display());
    Ok(())
}

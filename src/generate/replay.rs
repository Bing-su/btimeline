use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use path_slash::PathBufExt as _;
use serde_json::{Value, json};

use super::{Pull, Source, load_one, multi};
use crate::timeline::replay::{self, Evidence, Signal};

fn signals(source: &Source) -> Result<Vec<Signal>> {
    let master = &source.log.report.master_data;
    let actors: BTreeMap<_, _> = master.actors.iter().map(|a| (a.id, &a.name)).collect();
    let abilities: BTreeMap<_, _> = master
        .abilities
        .iter()
        .map(|a| (a.game_id, &a.name))
        .collect();
    let start = source
        .log
        .report
        .fights
        .first()
        .context("Missing fight")?
        .start_time;
    let mut result = Vec::new();
    // Replay all raw casts, including excluded/friendly/melee events, to expose accidental activation.
    for (index, event) in source.log.events.iter().enumerate() {
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
fn evidence(
    yaml: &str,
    report: &Value,
    source: &Source,
    peers: &BTreeMap<String, (Pull, String)>,
) -> Result<Evidence> {
    let timeline: Value = serde_saphyr::from_str(yaml)?;
    let entries: Vec<_> = timeline
        .get("entries")
        .context("Missing entries")?
        .as_array()
        .context("Missing entries")?
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry["kind"] == "event")
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
        report["group"] == serde_json::to_value(&source.key)?,
        "Replay input group differs from generation group"
    );
    let single = report.get("input").is_some();
    let mut relations = BTreeMap::new();
    let mut result = Evidence::default();
    for (&entry, slot) in entries.iter().zip(slots) {
        let samples = if single {
            vec![
                json!({"file":report.pointer("/input/file").context("Missing generation file")?, "eventIndices":[slot["eventIndex"]]}),
            ]
        } else {
            slot["samples"]
                .as_array()
                .context("Missing slot samples")?
                .clone()
        };
        let mut outcomes = Vec::new();
        for sample in &samples {
            let file = sample["file"].as_str().context("Missing sample file")?;
            let index = sample["eventIndices"]
                .as_array()
                .and_then(|indices| indices.first())
                .and_then(Value::as_u64)
                .context("Missing sample event index")? as usize;
            if !relations.contains_key(file) {
                let peer = peers
                    .get(file)
                    .map(|(pull, _)| pull)
                    .context("Sample file absent from generation inputs")?;
                if !same_file(file, &source.pull.file) {
                    relations.insert(
                        file.to_owned(),
                        multi::correspondence(peer, &source.pull)?.0,
                    );
                }
            }
            let peer = peers
                .get(file)
                .map(|(pull, _)| pull)
                .context("Missing evidence peer")?;
            let known = same_file(file, &source.pull.file);
            let relation = relations.get(file);
            let matched = if known {
                Some(index)
            } else {
                relation.and_then(|r| r.matched.get(&index)).copied()
            };
            let censored = !known
                && matched.is_none()
                && match relation {
                    Some(r) => {
                        r.censored.contains(&index) || after_end(peer, &source.pull, index, r)?
                    }
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
        let direct = samples.iter().find(|sample| {
            sample["file"]
                .as_str()
                .is_some_and(|file| same_file(file, &source.pull.file))
        });
        if let Some(sample) = direct {
            let index = sample["eventIndices"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(Value::as_u64)
                .context("Missing direct index")? as usize;
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
    let mut peers = BTreeMap::new();
    for item in inputs {
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
        if report.get("mode").and_then(Value::as_str) == Some("dungeon") {
            filter_boss_spans(&mut pull, item)?;
        }
        peers.insert(file.to_owned(), (pull, source.sha256));
    }
    Ok(peers)
}

fn filter_boss_spans(pull: &mut Pull, report: &Value) -> Result<()> {
    let spans = report["bossSegments"]
        .as_array()
        .context("Missing boss segments")?;
    pull.occurrences.retain(|row| {
        spans.iter().any(|span| {
            span["startMs"]
                .as_i64()
                .is_some_and(|at| row.relative_ms >= at)
                && span["endMs"]
                    .as_i64()
                    .is_some_and(|at| row.relative_ms <= at)
        })
    });
    Ok(())
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
    let group = multi::select_group(
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
    let mut pulls = Vec::new();
    for pull in &group.pulls {
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
        let signals = signals(&source)?;
        if report.get("mode").and_then(Value::as_str) == Some("dungeon") {
            let (_, spans) = super::draft::build_single(&source, super::GenerateMode::Dungeon)?;
            filter_boss_spans(&mut source.pull, &spans)?;
        }
        let evidence = evidence(&yaml, &report, &source, &peers)?;
        let represented: BTreeSet<_> = evidence.expected.values().flatten().copied().collect();
        let replay = replay::run(&yaml, &signals, source.pull.end_ms, &evidence)?;
        pulls.push(json!({"file":pull.file,"sha256":source.sha256,"report":pull.report,"fight":pull.fight,
            "revision":pull.revision,"logVersion":pull.log_version,"endMs":pull.end_ms,
            "termination":if pull.kill {"kill"} else {"wipe"},
            "unrepresentedEventIndices":source.pull.occurrences.iter().filter(|row| row.kind == "cast" && !represented.contains(&row.event_index)).map(|row| row.event_index).collect::<Vec<_>>(),
            "unfinishedStarts":pull.occurrences.iter().filter(|row| row.kind == "begincast" && row.completion_event_index.is_none()).map(|row| row.event_index).collect::<Vec<_>>(),
            "replay":replay}));
    }
    let passed = pulls
        .iter()
        .all(|pull| pull.pointer("/replay/passed").and_then(Value::as_bool) == Some(true));
    let result = json!({"status":if passed {"internally_validated"} else {"draft"},
        "toolVersion":env!("CARGO_PKG_VERSION"),"timeline":super::slash_path(timeline),
        "timelineSha256":super::sha256(yaml.as_bytes()),"evidenceSha256":super::sha256(&report_bytes),
        "evidence":super::slash_path(&report_path),"group":group.key,"pulls":pulls,
        "validation":{"schemaAndSemantic":true,"replayExecuted":true,"replay":passed,"cactbotParser":false,"runtime":false},
        "policy":{"clock":"independent zero at each fight start; sync corrections apply only after unique active matches",
            "window":"inclusive boundaries; absent window is +/-2500 ms",
            "ambiguity":"multiple active matches are reported, never resolved by entry order",
            "supportedSignals":["Ability", "StartsUsing"],
            "coverage":"Only represented rows are validated; disabled syncs and unrepresented casts are reported separately.",
            "unsupported":"ACT raw regex, unavailable network fields and simultaneous exit/forcejump priority remain unverified",
            "lookahead":"not executed; display needs actual runtime verification"}});
    let mut markdown = String::from(
        "# 재생 결과\n\n| 원본 | 종료 | 통과 | 오매칭 | 모호 | jump 오류 | 의존성 위반 | window 미검출 | 누락 | 종료 잘림 | 미지원 | 미검증 | 최대 시간 오차 (ms) |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for pull in &pulls {
        let s = pull
            .pointer("/replay/summary")
            .context("Missing replay summary")?;
        writeln!(
            markdown,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            pull["file"]
                .as_str()
                .context("Missing file")?
                .replace('|', "\\|")
                .replace(['\r', '\n'], " "),
            pull["termination"]
                .as_str()
                .context("Missing termination")?,
            pull.pointer("/replay/passed")
                .context("Missing replay result")?,
            s["wrongMatches"],
            s["ambiguousMatches"],
            s["wrongJumps"],
            s["dependencyViolations"],
            s["windowMisses"],
            s["missing"],
            s["censored"],
            s["unsupported"],
            s["unverified"],
            s["maxAbsErrorMs"]
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

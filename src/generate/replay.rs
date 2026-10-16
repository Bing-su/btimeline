use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use indicatif::{ParallelProgressIterator, ProgressStyle};
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
    excluded_from_generation: bool,
    excluded_event_indices: BTreeSet<usize>,
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
    replay_scope: &'static str,
    all_inputs_passed: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    groups: Option<Vec<GroupKey>>,
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
            // The arena opening ends combat, e.g. the controller stops before a boss retry.
            "instancesealupdate"
                if event.extra.get("sealType").and_then(Value::as_i64) == Some(3) =>
            {
                "CombatEnd"
            }
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

pub(super) fn same_file(file: &str, input: &str) -> bool {
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
            > i128::from(
                target
                    .party_defeat_ms
                    .unwrap_or(target.end_ms)
                    .min(target.end_ms),
            ),
    )
}

// Audit the first actual dispatch only, e.g. the later opposite beam cannot activate both paths in the oracle.
fn select_paths(
    report: &Value,
    entries: &[usize],
    pull: &Pull,
    result: &mut Evidence,
) -> Result<()> {
    if let Some(branches) = report
        .pointer("/extensions/branches")
        .and_then(Value::as_array)
    {
        for branch in branches {
            let paths = branch["paths"].as_array().context("Missing branch paths")?;
            if branch["packetWave"] == true {
                for path in paths {
                    if let Some(entry) = path["selectorSlot"]
                        .as_u64()
                        .and_then(|i| entries.get(i as usize))
                    {
                        result.packet_wave_selectors.insert(*entry);
                    }
                }
            }
            let first = paths
                .iter()
                .filter_map(|path| {
                    let entry = entries.get(path["selectorSlot"].as_u64()? as usize)?;
                    result.expected.get(entry)?.first().copied()
                })
                // Raw arrays can be unsorted, e.g. an earlier helper start appears after the main cast.
                .min_by_key(|index| {
                    pull.occurrences
                        .iter()
                        .find(|row| row.event_index == *index)
                        .map(|row| (row.relative_ms, *index))
                });
            let selected: Vec<_> = paths
                .iter()
                .filter(|path| {
                    path["selectorSlot"]
                        .as_u64()
                        .and_then(|i| entries.get(i as usize))
                        .is_some_and(|entry| {
                            result
                                .expected
                                .get(entry)
                                .and_then(|indices| indices.first())
                                .copied()
                                == first
                                && first.is_some()
                        })
                })
                .collect();
            if !selected.is_empty() {
                for path in paths.iter().filter(|path| !selected.contains(path)) {
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
    Ok(())
}

// Correspondence audits activation; the independent replay clock never reads it to choose a sync.
pub(super) fn evidence(
    yaml: &str,
    report: &Value,
    key: &GroupKey,
    pull: &Pull,
    log: &crate::fflogs::model::CollectedLog,
    peers: &BTreeMap<&str, &Pull>,
) -> Result<Evidence> {
    if report.get("segments").is_some() {
        return super::combined::evidence(yaml, report, key, pull, log, peers);
    }
    if report.pointer("/repeats/accepted").and_then(Value::as_bool) == Some(true) {
        return super::repeat::fold_evidence(yaml, report, key, pull, log, peers);
    }
    // Audit phase schedules against independent source times, e.g. a retry reuses the same mechanic rows.
    if report.get("schedule").is_some() {
        ensure!(
            report.get("group") == Some(&serde_json::to_value(key)?),
            "Schedule input group differs from generation group"
        );
        return super::schedule::evidence(yaml, report, pull, log);
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
    // Empty consensus cannot validate mechanics, e.g. incompatible casts must not pass prepare.
    ensure!(
        !entries.is_empty(),
        "No generated event rows to replay; inspect the generation report"
    );
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
    let actors = log
        .report
        .master_data
        .actors
        .iter()
        .map(|actor| (actor.id, actor.name.clone()))
        .collect();
    // Only multi-pull raid rows use merged wire casts; single drafts and dungeon retries retain raw representatives.
    let representatives: BTreeMap<_, _> = if !single && report["mode"] == "raid" {
        multi::wire_signals(pull, log, &actors)?
            .iter()
            .flat_map(|(&index, signal)| {
                signal
                    .event_indices
                    .iter()
                    .map(move |&event| (event, index))
            })
            .collect()
    } else {
        BTreeMap::new()
    };
    let mut relations = BTreeMap::new();
    let mut result = Evidence::default();
    for (&entry, slot) in entries.iter().zip(slots) {
        let alternative_ids = slot["abilityIds"].as_array().filter(|ids| ids.len() > 1);
        if slot["evidence"] == "unorderedPacketWave" {
            result.unordered.insert(entry);
        }
        let starts = matches!(timeline.entries.get(entry),
            Some(crate::timeline::Entry::Event { sync: Some(crate::timeline::Sync::Network(sync)), .. })
                if matches!(sync.log, crate::timeline::LogType::StartsUsing));
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
                let mapped: BTreeSet<_> = sample
                    .event_indices
                    .iter()
                    .filter_map(|index| relation?.matched.get(index))
                    .map(|index| representatives.get(index).copied().unwrap_or(*index))
                    .collect();
                (mapped.len() == 1)
                    .then(|| mapped.into_iter().next())
                    .flatten()
            }
            .filter(|target| {
                // A dispatch observes its literal ID, e.g. P→Q alignment cannot select Q after a real P.
                slot["selector"] != true
                    || peer
                        .occurrences
                        .iter()
                        .find(|row| row.event_index == index)
                        .zip(
                            pull.occurrences
                                .iter()
                                .find(|row| row.event_index == *target),
                        )
                        .is_some_and(|(source, target)| {
                            source.ability_id == target.ability_id
                                || alternative_ids.is_some_and(|ids| {
                                    ids.iter().any(|id| id.as_i64() == Some(target.ability_id))
                                })
                        })
            });
            let matched =
                matched.map(|index| representatives.get(&index).copied().unwrap_or(index));
            let censored = !known
                && matched.is_none()
                && match relation {
                    Some(r) => r.censored.contains(&index) || after_end(peer, pull, index, r)?,
                    None => false,
                };
            ensure!(
                peer.occurrences.iter().any(|row| row.event_index == index
                    && row.kind == if starts { "begincast" } else { "cast" }),
                "Evidence refers to a non-cast occurrence"
            );
            outcomes.push((matched, censored));
        }
        // Known training samples are exact evidence; other peers do not override their source index.
        let direct = samples
            .iter()
            .find(|sample| known_files.contains(sample.file.as_str()));
        if matches!(
            slot["evidence"].as_str(),
            Some("conditionalInstantHelper" | "unresolvedPath" | "observedSuffix")
        ) && direct.is_none()
        {
            // Optional reactions need no activation, e.g. a clean missile dodge has no Launch record.
            result.inactive.insert(entry);
            continue;
        }
        if let Some(sample) = direct {
            let index = *sample
                .event_indices
                .first()
                .context("Missing direct index")?;
            let index = representatives.get(&index).copied().unwrap_or(index);
            result.expected.insert(entry, BTreeSet::from([index]));
        } else {
            let matches: BTreeSet<_> = outcomes.iter().filter_map(|(index, _)| *index).collect();
            if matches.len() == 1
                && (alternative_ids.is_some()
                    || outcomes
                        .iter()
                        .all(|(index, censored)| index.is_some() || *censored))
            {
                // An ID array is an alternative, e.g. mixed true/fake packets need not match every training variant.
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
    // Select each attempt separately, e.g. the merge on an inactive sibling is never required.
    select_paths(report, &entries, pull, &mut result)?;
    if report["mode"] != "raid" {
        let fight = log.report.fights.first().context("Missing retry fight")?;
        let enemies = fight
            .enemy_npcs
            .iter()
            .chain(&fight.enemy_pets)
            .map(|enemy| enemy.id)
            .collect();
        let (spans, attempts) = super::draft::boss_spans(log, &enemies);
        let selected_spans = super::sections::merge_spans(
            spans
                .values()
                .map(|&(lo, hi)| (lo - fight.start_time, hi - fight.start_time))
                .collect(),
        );
        for attempt in attempts.iter().filter(|attempt| !attempt.selected) {
            let &(start, _) = spans
                .get(&attempt.actor_id)
                .context("Missing selected attempt")?;
            let section = selected_spans
                .iter()
                .position(|&(lo, hi)| {
                    lo <= start - fight.start_time && start - fight.start_time <= hi
                })
                .context("Missing retry section")?;
            let mut target = pull.clone();
            target.occurrences = pull
                .raw_occurrences
                .as_ref()
                .unwrap_or(&pull.occurrences)
                .iter()
                .filter(|row| {
                    attempt.start_ms <= row.relative_ms && row.relative_ms <= attempt.end_ms
                })
                .cloned()
                .collect();
            target.boss_spans = vec![(attempt.start_ms, attempt.end_ms)];
            target.kill = false;
            target.end_ms = attempt.end_ms;
            let mut relations = BTreeMap::new();
            for (&file, peer) in peers {
                let peer_spans = if peer.boss_spans.is_empty() {
                    &selected_spans
                } else {
                    &peer.boss_spans
                };
                let Some(&(lo, hi)) = peer_spans.get(section) else {
                    continue;
                };
                let mut reference = (*peer).clone();
                reference.occurrences = reference
                    .occurrences
                    .iter()
                    .filter(|row| lo <= row.relative_ms && row.relative_ms <= hi)
                    .cloned()
                    .collect();
                reference.boss_spans = vec![(lo, hi)];
                reference.end_ms = hi;
                relations.insert(file, multi::correspondence(&reference, &target)?.0);
            }
            let mut retry = Evidence::default();
            if single {
                // Relations use training indices, e.g. holdout index 2 maps from the slot's training index 0.
                for (&entry, slot) in entries.iter().zip(slots) {
                    let index = serde_json::from_value::<usize>(slot["eventIndex"].clone())
                        .context("Missing retry sample event index")?;
                    for relation in relations.values() {
                        if let Some(index) = relation.matched.get(&index) {
                            retry.expected.entry(entry).or_default().insert(*index);
                        }
                    }
                }
            }
            // A retry can choose another observed path, e.g. left beam before a right-beam clear.
            for (&entry, slot) in entries.iter().zip(slots) {
                if let Some(samples) = slot["samples"].as_array() {
                    for sample in samples {
                        if let Some(index) = sample["file"]
                            .as_str()
                            .and_then(|file| relations.get(file))
                            .and_then(|relation| {
                                sample["eventIndices"]
                                    .as_array()?
                                    .first()?
                                    .as_u64()
                                    .and_then(|index| relation.matched.get(&(index as usize)))
                            })
                            && target.occurrences.iter().any(|row| {
                                row.event_index == *index
                                    && Some(row.ability_id)
                                        == sample["abilityId"]
                                            .as_i64()
                                            .or_else(|| slot["abilityId"].as_i64())
                            })
                        {
                            retry.expected.entry(entry).or_default().insert(*index);
                        }
                    }
                }
            }
            select_paths(report, &entries, &target, &mut retry)?;
            for (entry, indices) in retry.expected {
                result.expected.entry(entry).or_default().extend(indices);
                result.censored.remove(&entry);
                result.missing.remove(&entry);
                result.inactive.remove(&entry);
            }
        }
    }
    // An oracle-selected path only audits activation; absent siblings must still fail if they match.
    // Example: the shared merge cast belongs to the chosen X→A path, not also to Y→B.

    // Consumer fan-out remains valid on holdouts, e.g. eight target packets share the first packet's clock correction.
    let mut packet_pull = pull.clone();
    if let Some(raw) = packet_pull.raw_occurrences.take() {
        packet_pull.occurrences = raw;
    }
    let packets: Vec<_> = if !single && report["mode"] == "raid" {
        multi::wire_signals(&packet_pull, log, &actors)?
            .into_values()
            .collect()
    } else {
        super::alignment::signals(&packet_pull)
    };
    for (&entry, expected) in &result.expected {
        let observed = packets
            .iter()
            .filter(|signal| {
                signal
                    .event_indices
                    .iter()
                    .any(|index| expected.contains(index))
            })
            .flat_map(|signal| signal.event_indices.iter().copied())
            .collect();
        result.observed_packets.insert(entry, observed);
    }

    result.lookahead_ms = report
        .pointer("/extensions/lookaheadMs")
        .and_then(Value::as_i64);
    Ok(result)
}

fn load_peers(report: &Value) -> Result<BTreeMap<String, (Pull, String)>> {
    if let Some(segments) = report.get("segments").and_then(Value::as_array) {
        let mut peers = BTreeMap::new();
        for segment in segments {
            peers.extend(load_peers(
                segment
                    .get("report")
                    .context("Missing checkpoint evidence")?,
            )?);
        }
        return Ok(peers);
    }
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
    let keys: Vec<GroupKey> = if let Some(groups) = report.get("groups") {
        serde_json::from_value(groups.clone())?
    } else {
        vec![serde_json::from_value(
            report
                .get("group")
                .context("Missing generation group")?
                .clone(),
        )?]
    };
    let groups = input::select_groups(input.as_ref(), None, None, None)?;
    let group = super::Group {
        key: keys.first().context("Missing replay group")?.clone(),
        pulls: groups
            .into_iter()
            .filter(|g| keys.contains(&g.key))
            .flat_map(|g| g.pulls)
            .collect(),
    };
    ensure!(
        !group.pulls.is_empty(),
        "Replay input group differs from generation group"
    );
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
            let source_report = super::combined::source_report(&report, &source.key)?;
            let exclusion = report
                .pointer("/inputSelection/excluded")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find(|item| {
                        item["file"]
                            .as_str()
                            .is_some_and(|file| same_file(file, &pull.file))
                    })
                });
            if let Some(exclusion) = exclusion {
                ensure!(
                    exclusion["sha256"] == source.sha256,
                    "Excluded input bytes changed: {}",
                    pull.file
                );
            }
            if let Some((_, hash)) = peers
                .values()
                .find(|(peer, _)| same_file(&peer.file, &source.pull.file))
            {
                ensure!(
                    hash == &source.sha256,
                    "Replay input changed after evidence inspection"
                );
            }
            let mut signals = signals(&source.log)?;
            if source_report.get("schedule").is_some() {
                // A recorded arena reload stops the consumer clock, e.g. a merged ultimate log restarts at P1.
                for index in
                    super::schedule::reset_indices(source_report, &source.pull, &source.log)?
                {
                    let event = source
                        .log
                        .events
                        .get(index)
                        .context("Missing schedule reset")?;
                    signals.push(Signal {
                        index,
                        at_ms: event.timestamp
                            - source
                                .log
                                .report
                                .fights
                                .first()
                                .context("Missing schedule fight")?
                                .start_time,
                        log: "CombatEnd",
                        fields: BTreeMap::new(),
                    });
                }
                signals.sort_by_key(|signal| (signal.at_ms, signal.index));
            }
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
            let mut represented: BTreeSet<_> =
                evidence.expected.values().flatten().copied().collect();
            let mut excluded = if source_report.get("schedule").is_some() {
                super::schedule::excluded_events(source_report, &source.pull, &source.log)?
            } else {
                BTreeSet::new()
            };
            represented.extend(evidence.observed_packets.values().flatten());
            // Coverage includes the whole wave; activation still uses one representative, e.g. four Explosions.
            for signal in super::alignment::signals(&source.pull) {
                if signal
                    .event_indices
                    .first()
                    .is_some_and(|index| represented.contains(index))
                {
                    represented.extend(signal.event_indices);
                }
            }
            if let Some(coverage) = report
                .get("outputCoverage")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items.iter().find(|item| {
                        item["file"]
                            .as_str()
                            .is_some_and(|file| same_file(file, &source.pull.file))
                    })
                })
            {
                represented.extend(serde_json::from_value::<BTreeSet<usize>>(
                    coverage["representedEventIndices"].clone(),
                )?);
                if let Some(indices) = coverage.get("excludedEventIndices") {
                    excluded.extend(serde_json::from_value::<BTreeSet<usize>>(indices.clone())?);
                }
            }
            excluded.retain(|index| !represented.contains(index));
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
                excluded_from_generation: exclusion.is_some(),
                unrepresented_event_indices: source
                    .pull
                    .raw_occurrences
                    .as_ref()
                    .unwrap_or(&source.pull.occurrences)
                    .iter()
                    .filter(|row| {
                        row.kind == "cast"
                            && !represented.contains(&row.event_index)
                            && !excluded.contains(&row.event_index)
                    })
                    .map(|row| row.event_index)
                    .collect(),
                excluded_event_indices: excluded,
                unfinished_starts: pull
                    .occurrences
                    .iter()
                    .filter(|row| row.kind == "begincast" && row.completion_event_index.is_none())
                    .map(|row| row.event_index)
                    .collect(),
                replay,
            })
        })
        .progress_with_style(ProgressStyle::with_template(
            "Replaying pulls {wide_bar} {pos}/{len}",
        )?)
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    // Excluded files are diagnostic holdouts, e.g. an unseen wipe reaction cannot reject a clean-clear draft.
    let passed = pulls
        .iter()
        .filter(|pull| !pull.excluded_from_generation)
        .all(|pull| pull.replay.passed);
    let all_inputs_passed = pulls.iter().all(|pull| pull.replay.passed);
    let result = ReplayReport {
        status: if all_inputs_passed {
            "internally_validated"
        } else if passed {
            "selected_inputs_validated"
        } else {
            "draft"
        },
        tool_version: env!("CARGO_PKG_VERSION"),
        timeline: super::slash_path(timeline),
        timeline_sha256: super::sha256(yaml.as_bytes()),
        evidence_sha256: super::sha256(&report_bytes),
        evidence: super::slash_path(&report_path),
        group: &group.key,
        groups: report
            .get("groups")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?,
        pulls: &pulls,
        validation: ReplayValidation {
            validation: super::report::Validation {
                replay: passed,
                ..Default::default()
            },
            replay_executed: true,
            replay_scope: if pulls.iter().any(|pull| pull.excluded_from_generation) {
                "selected generation inputs; excluded files replayed diagnostically"
            } else {
                "all replay inputs"
            },
            all_inputs_passed,
        },
        policy: ReplayPolicy {
            clock: "fight start assumes the declared InCombat entry signal; raw cast replay starts independently at zero and audits unique sync corrections",
            window: "lower boundary inclusive, upper boundary exclusive; absent window is +/-2500 ms",
            ambiguity: "multiple active matches are reported, never resolved by entry order",
            supported_signals: ["Ability", "StartsUsing"],
            coverage: "Only represented rows are validated; intentionally excluded events and unrepresented casts are separate. Files excluded by inputSelection are diagnostic only; allInputsPassed records their result too.",
            unsupported: "ACT raw regex, unavailable network fields and simultaneous exit/forcejump priority remain unverified",
            lookahead: "independent projection before/after sync jumps; actual runtime display not executed",
        },
    };
    let mut markdown = String::from(
        "# 재생 결과\n\n| 원본 | 생성 표본 | 종료 | 통과 | 오매칭 | 모호 | jump 오류 | 의존성 위반 | window 미검출 | 누락 | 종료 잘림 | 미지원 | 미검증 | 최대 시간 오차 (ms) |\n| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
    );
    for pull in &pulls {
        let s = &pull.replay.summary;
        writeln!(
            markdown,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            pull.file.replace('|', "\\|").replace(['\r', '\n'], " "),
            if pull.excluded_from_generation {
                "제외 · 진단용"
            } else {
                "검증 대상"
            },
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

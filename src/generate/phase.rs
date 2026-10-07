use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::alignment::{Signal, SignalKey};
use super::multi::{Input, field_pattern, statistics};
use super::report::TimeStatistics;
use super::{draft, replay};
use crate::timeline::{Destination, Entry, Jump, JumpWhen, LogType, NetworkSync, Sync, Timeline};

type Samples<'a> = Vec<(usize, &'a Signal)>;
type Paths<'a> = Vec<Vec<(usize, Vec<&'a Signal>)>>;

// Keep compiled evidence typed until serialization, e.g. common slots omit path-only fields.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseSlot {
    id: usize,
    block: usize,
    at_ms: serde_json::Number,
    time: TimeStatistics,
    absolute_time: TimeStatistics,
    ability_ids: Vec<i64>,
    samples: Vec<PhaseSample>,
    unobserved_after_wipe: Vec<String>,
    evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clock_time: Option<TimeStatistics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window_ms: Option<[i64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alignment_slot: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selector: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transition_clock_range: Option<TimeStatistics>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseSample {
    file: String,
    event_indices: Vec<usize>,
    instance_ids: Vec<i64>,
    ability_id: i64,
    time_ms: i64,
    block_entry_ms: i64,
    relative_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhaseBlock {
    id: usize,
    entry: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    draft_entry_ms: i64,
    time: TimeStatistics,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchPath {
    label: String,
    entry_ms: i64,
    merge_label: String,
    selector_slot: usize,
    slots: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Branch {
    id: usize,
    merge_label: String,
    merge_ms: i64,
    paths: Vec<BranchPath>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Phase {
    label: String,
    entry_ms: i64,
    alignment_slot: usize,
    selector_slot: usize,
}

#[derive(Serialize)]
struct ReplayCheck {
    file: String,
    passed: bool,
    summary: crate::timeline::replay::Summary,
    jumps: Vec<crate::timeline::replay::JumpTrace>,
    previews: Vec<crate::timeline::replay::Preview>,
}

// Preserve rejection diagnostics without accepted-only fields, e.g. branches are absent on failure.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RejectedExtension {
    accepted: bool,
    reason: String,
    lookahead_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    checks: Option<Vec<ReplayCheck>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptedExtension {
    accepted: bool,
    lookahead_ms: i64,
    max_observed_end_ms: i64,
    branches: Vec<Branch>,
    phases: Vec<Phase>,
    forcejump_generated: bool,
    preview_projection_executed: bool,
    actual_display_executed: bool,
    preview: &'static str,
    checks: Vec<ReplayCheck>,
}

// Preserve a shared prefix before selecting a branch, e.g. A→U→X,P versus A→U→Y,Q.
fn common_prefix(paths: &Paths<'_>) -> usize {
    let length = paths
        .iter()
        .flatten()
        .map(|(_, casts)| casts.len())
        .min()
        .unwrap_or(0);
    (0..length)
        .take_while(|&position| {
            let keys: BTreeSet<_> = paths
                .iter()
                .flatten()
                .filter_map(|(_, casts)| casts.get(position).map(|s| &s.key))
                .collect();
            keys.len() == 1
        })
        .count()
}

// Check JSON evidence objects before updates, e.g. a slot must never become a scalar silently.
pub(super) fn put(field: impl Serialize, value: &mut Value, key: &str) -> Result<()> {
    value
        .as_object_mut()
        .context("Expected evidence object")?
        .insert(key.into(), serde_json::to_value(field)?);
    Ok(())
}

// Keep one observed clock per pull, e.g. a 20s phase delay is measured after the last successful sync.
struct Compiler<'a> {
    inputs: &'a [Input],
    clocks: Vec<(i64, i64)>,
    origins: Vec<i64>,
    rows: Vec<(Entry, Option<PhaseSlot>)>,
    blocks: Vec<PhaseBlock>,
    branches: Vec<Branch>,
    phases: Vec<Phase>,
    horizon: i64,
    guard: i64,
    next: i64,
}

fn time(entry: &Entry) -> f64 {
    match entry {
        Entry::Event { at, .. } | Entry::Label { at, .. } => *at,
        _ => 0.0,
    }
}

pub(super) fn sample_index(slot: &Value, file: &str) -> Option<usize> {
    slot["samples"]
        .as_array()?
        .iter()
        .find(|s| s["file"] == file)?["eventIndices"]
        .as_array()?
        .first()?
        .as_u64()
        .map(|i| i as usize)
}

fn samples<'a>(slot: &Value, inputs: &'a [Input]) -> Samples<'a> {
    inputs
        .iter()
        .enumerate()
        .filter_map(|(i, input)| {
            Some((
                i,
                input.signals.get(&sample_index(slot, &input.pull.file)?)?,
            ))
        })
        .collect()
}

// Only fully observed, direction-independent endpoints establish a branch; wipes supply no new path.
// Example: A→X,P→C and A→Y,Q→C; A→X followed by a wipe is not an empty alternative.
fn paths<'a>(before: Option<&Value>, after: &Value, inputs: &'a [Input]) -> Result<Paths<'a>> {
    let mut variants: BTreeMap<Vec<SignalKey>, Vec<(usize, Vec<&Signal>)>> = BTreeMap::new();
    for (i, input) in inputs.iter().enumerate() {
        let Some(end) = sample_index(after, &input.pull.file) else {
            continue;
        };
        let start = before.and_then(|slot| sample_index(slot, &input.pull.file));
        if before.is_some() && start.is_none() {
            return Ok(Vec::new());
        }
        let ordered: Vec<_> = input.signals.values().collect();
        let mut ordered = ordered;
        ordered.sort_by_key(|s| (s.time_ms, s.event_indices.first().copied()));
        let lo = start
            .and_then(|start| {
                ordered
                    .iter()
                    .position(|s| s.event_indices.first() == Some(&start))
            })
            .map_or(0, |position| position + 1);
        let hi = ordered
            .iter()
            .position(|s| s.event_indices.first() == Some(&end))
            .context("Missing branch endpoint")?;
        let signals = ordered.get(lo..=hi).context("Invalid branch interval")?;
        let keys = signals.iter().map(|s| s.key.clone()).collect();
        let casts = signals
            .iter()
            .filter(|s| s.key.kind == "cast")
            .copied()
            .collect();
        variants.entry(keys).or_default().push((i, casts));
    }
    let result: Paths<'a> = variants.into_values().collect();
    if result.len() < 2
        || result
            .iter()
            .any(|path| path.iter().any(|(_, casts)| casts.len() < 2))
    {
        return Ok(Vec::new());
    }
    let prefix = common_prefix(&result);
    let selectors: BTreeSet<_> = result
        .iter()
        .filter_map(|path| {
            path.first()?
                .1
                .get(prefix)
                .map(|s| (s.key.actor_game_id, s.key.ability_id, &s.key.role))
        })
        .collect();
    // Nested choices need more evidence, e.g. X→P, X→Q and Y→R cannot share one dispatch.
    if selectors.len() != result.len() {
        return Ok(Vec::new());
    }
    Ok(result)
}

impl<'a> Compiler<'a> {
    fn label(&mut self, name: &str, at: i64, samples: &Samples<'a>) -> Result<()> {
        self.rows.push((
            Entry::Label {
                at: at as f64 / 1000.0,
                name: name.into(),
            },
            None,
        ));
        self.blocks.push(PhaseBlock {
            id: self.blocks.len(),
            entry: "observedSyncJump",
            label: Some(name.into()),
            draft_entry_ms: at,
            time: statistics(samples.iter().map(|(_, s)| s.time_ms).collect())?,
        });
        Ok(())
    }

    fn allocate(&mut self, occupied: i64) -> Result<i64> {
        // Separate blocks even if an exit is missing until the longest observed pull ends.
        // ponytail: isolation is bounded by observed duration + lookahead; longer holdouts require replay.
        self.next = self
            .next
            .max(occupied)
            .checked_add(self.guard)
            .context("Virtual block time overflow")?;
        ensure!(
            self.next <= i64::MAX / 4,
            "Virtual block exceeds replay clock range"
        );
        Ok(self.next)
    }

    fn emit(
        &mut self,
        mut entry: Entry,
        samples: &Samples<'a>,
        mut slot: PhaseSlot,
        target: Option<&str>,
    ) -> Result<i64> {
        ensure!(!samples.is_empty(), "No phase timing samples");
        let clocks: Vec<_> = samples
            .iter()
            .map(|(i, s)| {
                self.clocks
                    .get(*i)
                    .and_then(|(wall, clock)| clock.checked_add(s.time_ms - wall))
                    .context("Missing or overflowing pull clock")
            })
            .collect::<Result<_>>()?;
        let timing = statistics(clocks.clone())?;
        let at = (timing.median_ms / 100.0).round() as i64 * 100;
        // Round outward at SPEC precision, e.g. 51ms of drift requires a 0.1s window.
        let before = (((at - timing.min_ms).max(0) + 99) / 100 * 100).max(2500);
        let after = (((timing.max_ms - at).max(0) + 99) / 100 * 100).max(2500);
        let Entry::Event {
            at: entry_at,
            sync,
            jump,
            ..
        } = &mut entry
        else {
            anyhow::bail!("Missing phase event")
        };
        *entry_at = at as f64 / 1000.0;
        let Some(Sync::Network(sync)) = sync else {
            anyhow::bail!("Missing phase sync")
        };
        sync.window = Some([before as f64 / 1000.0, after as f64 / 1000.0]);
        if let Some(target) = target {
            ensure!(
                sync.enabled,
                "Branch needs an enabled discriminator and merge sync"
            );
            *jump = Some(Jump {
                to: Destination::Label(target.into()),
                when: JumpWhen::Sync,
            });
        }
        slot.at_ms = at.into();
        slot.clock_time = Some(timing);
        slot.window_ms = Some([before, after]);
        let relative = samples
            .iter()
            .map(|(i, s)| {
                self.origins
                    .get(*i)
                    .map(|wall| s.time_ms - wall)
                    .context("Missing timing origin")
            })
            .collect::<Result<Vec<_>>>()?;
        slot.time = statistics(relative)?;
        for sample in &mut slot.samples {
            let i = self
                .inputs
                .iter()
                .position(|input| input.pull.file == sample.file)
                .context("Missing sample input")?;
            let wall = self.origins.get(i).context("Missing sample origin")?;
            sample.block_entry_ms = *wall;
            sample.relative_ms = sample.time_ms - wall;
        }
        self.rows.push((entry, Some(slot)));
        if sync_enabled(self.rows.last().context("Missing emitted row")?) {
            for (i, s) in samples {
                *self.clocks.get_mut(*i).context("Missing pull clock")? = (s.time_ms, at);
            }
        }
        Ok(at + after)
    }

    fn branch_entry(&self, samples: &Samples<'a>) -> Result<(Entry, PhaseSlot)> {
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut refs = Vec::new();
        let mut name = None;
        for (i, signal) in samples {
            let input = self.inputs.get(*i).context("Missing branch input")?;
            ids.insert(signal.key.ability_id);
            for &j in &signal.event_indices {
                let event = input.log.events.get(j).context("Missing branch event")?;
                let source = event
                    .source_id
                    .and_then(|id| input.actors.get(&id))
                    .context("Missing branch source")?;
                ensure!(
                    !source.contains(['#', '"', '\r', '\n']),
                    "Unsafe branch sync source"
                );
                names.insert(source.clone());
            }
            name = input
                .log
                .report
                .master_data
                .abilities
                .iter()
                .find(|a| a.game_id == signal.key.ability_id)
                .map(|a| a.name.clone());
            refs.push(PhaseSample {
                file: input.pull.file.clone(),
                event_indices: signal.event_indices.clone(),
                instance_ids: signal.instance_ids.clone(),
                ability_id: signal.key.ability_id,
                time_ms: signal.time_ms,
                block_entry_ms: samples.first().context("Missing branch samples")?.1.time_ms,
                relative_ms: 0,
            });
        }
        let fields = BTreeMap::from([
            (
                "id".into(),
                field_pattern(ids.iter().map(|id| format!("^{id:X}$")).collect())?,
            ),
            (
                "source".into(),
                field_pattern(
                    names
                        .iter()
                        .map(|s| format!("^{}$", regress::escape(s)))
                        .collect(),
                )?,
            ),
        ]);
        let times = statistics(samples.iter().map(|(_, s)| s.time_ms).collect())?;
        Ok((
            Entry::Event {
                at: 0.0,
                name: name.context("Missing branch ability")?,
                duration: None,
                sync: Some(Sync::Network(NetworkSync {
                    log: LogType::Ability,
                    fields,
                    enabled: true,
                    window: None,
                })),
                jump: None,
                note: None,
            },
            PhaseSlot {
                id: 0,
                block: 0,
                at_ms: 0.into(),
                time: times,
                absolute_time: times,
                ability_ids: ids.into_iter().collect(),
                samples: refs,
                unobserved_after_wipe: Vec::new(),
                evidence: "discriminatedPath".into(),
                clock_time: None,
                window_ms: None,
                alignment_slot: None,
                path: None,
                selector: None,
                transition_clock_range: None,
            },
        ))
    }

    fn branch(&mut self, variants: &Paths<'a>) -> Result<()> {
        let prefix = common_prefix(variants);
        for position in 0..prefix {
            let refs: Samples<'_> = variants
                .iter()
                .flatten()
                .filter_map(|(i, casts)| Some((*i, *casts.get(position)?)))
                .collect();
            let (entry, mut slot) = self.branch_entry(&refs)?;
            slot.evidence = "observedCommonPrefix".into();
            self.emit(entry, &refs, slot, None)?;
        }
        let variants: Paths<'_> = variants
            .iter()
            .map(|path| {
                path.iter()
                    .map(|(i, casts)| {
                        casts
                            .get(prefix..)
                            .map(|suffix| (*i, suffix.to_vec()))
                            .context("Invalid common prefix")
                    })
                    .collect::<Result<_>>()
            })
            .collect::<Result<_>>()?;
        let group = self.branches.len();
        let merge_label = format!("branch-{group}-merge");
        let original_clocks = self.clocks.clone();
        let arrivals = variants
            .iter()
            .flatten()
            .map(|(i, casts)| {
                let signal = casts.first().context("Missing path discriminator")?;
                let (wall, clock) = original_clocks
                    .get(*i)
                    .context("Missing discriminator clock")?;
                clock
                    .checked_add(signal.time_ms - wall)
                    .context("Discriminator clock overflow")
            })
            .collect::<Result<Vec<_>>>()?;
        let envelope = statistics(arrivals)?;
        let mut selectors = Vec::new();
        let mut occupied = self.next;
        for (path_id, path) in variants.iter().enumerate() {
            let refs: Samples<'_> = path
                .iter()
                .filter_map(|(i, casts)| Some((*i, *casts.first()?)))
                .collect();
            self.clocks.clone_from(&original_clocks);
            let (entry, mut slot) = self.branch_entry(&refs)?;
            let label = format!("branch-{group}-{path_id}");
            slot.path = Some(label.clone());
            slot.selector = Some(true);
            occupied = occupied.max(self.emit(entry, &refs, slot, Some(&label))?);
            // All choices are observable in the fork's arrival range, e.g. X at 4s or Y at 10s.
            // Preserve that envelope independently of each path's nominal selector time.
            let (entry, slot) = self.rows.last_mut().context("Missing selector row")?;
            if let Entry::Event {
                at,
                sync: Some(Sync::Network(sync)),
                ..
            } = entry
            {
                let at_ms = (*at * 1000.0).round() as i64;
                let before = (((at_ms - envelope.min_ms).max(0) + 99) / 100 * 100).max(2500);
                let after = (((envelope.max_ms - at_ms).max(0) + 99) / 100 * 100).max(2500);
                sync.window = Some([before as f64 / 1000.0, after as f64 / 1000.0]);
                let slot = slot.as_mut().context("Missing selector slot")?;
                slot.window_ms = Some([before, after]);
                slot.transition_clock_range = Some(envelope);
                occupied = occupied.max(at_ms + after);
            }
            selectors.push((label, refs));
        }
        let mut path_reports = Vec::new();
        let mut merges = Vec::new();
        for (path_id, path) in variants.iter().enumerate() {
            let (label, refs) = selectors.get(path_id).context("Missing branch selector")?;
            let base = self.allocate(occupied)?;
            self.label(label, base, refs)?;
            for (i, s) in refs {
                *self.clocks.get_mut(*i).context("Missing branch clock")? = (s.time_ms, base);
                *self.origins.get_mut(*i).context("Missing branch origin")? = s.time_ms;
            }
            let length = path.first().context("Empty branch")?.1.len();
            for position in 1..length {
                let refs: Samples<'_> = path
                    .iter()
                    .filter_map(|(i, casts)| Some((*i, *casts.get(position)?)))
                    .collect();
                ensure!(refs.len() == path.len(), "Branch path lengths differ");
                let (entry, mut slot) = self.branch_entry(&refs)?;
                slot.path = Some(label.clone());
                let is_merge = position + 1 == length;
                occupied = occupied.max(self.emit(
                    entry,
                    &refs,
                    slot,
                    is_merge.then_some(merge_label.as_str()),
                )?);
                if is_merge {
                    merges.extend(refs);
                }
            }
            path_reports.push(BranchPath {
                label: label.clone(),
                entry_ms: base,
                merge_label: merge_label.clone(),
                selector_slot: 0,
                slots: Vec::new(),
            });
        }
        let merge_at = self.allocate(occupied)?;
        self.label(&merge_label, merge_at, &merges)?;
        for (i, s) in &merges {
            *self.clocks.get_mut(*i).context("Missing merge clock")? = (s.time_ms, merge_at);
            *self.origins.get_mut(*i).context("Missing merge origin")? = s.time_ms;
        }
        self.branches.push(Branch {
            id: group,
            merge_label,
            merge_ms: merge_at,
            paths: path_reports,
        });
        Ok(())
    }
}

fn sync_enabled(row: &(Entry, Option<PhaseSlot>)) -> bool {
    matches!(&row.0, Entry::Event { sync:Some(Sync::Network(sync)), .. } if sync.enabled)
}

pub(super) fn expand(
    yaml: String,
    report: Value,
    inputs: &[Input],
    lookahead: f64,
) -> Result<(String, Value)> {
    let (yaml, report) = compile(yaml, report, inputs, lookahead, true)?;
    if report
        .pointer("/extensions/accepted")
        .and_then(Value::as_bool)
        == Some(false)
    {
        // A rejected branch must not discard a safe common transition, e.g. an ambiguous X/Y before phase 2.
        let rejected = report
            .get("extensions")
            .context("Missing rejected candidate")?
            .clone();
        let (phase_yaml, mut phase_report) =
            compile(yaml.clone(), report.clone(), inputs, lookahead, false)?;
        if phase_report
            .pointer("/extensions/accepted")
            .and_then(Value::as_bool)
            == Some(true)
        {
            put(
                rejected,
                phase_report
                    .get_mut("extensions")
                    .context("Missing phase evidence")?,
                "rejectedBranchCandidate",
            )?;
            return Ok((phase_yaml, phase_report));
        }
    }
    Ok((yaml, report))
}

fn compile(
    yaml: String,
    report: Value,
    inputs: &[Input],
    lookahead: f64,
    branches: bool,
) -> Result<(String, Value)> {
    let timeline: Timeline = serde_saphyr::from_str(&yaml)?;
    let mut events = Vec::new();
    let mut other = Vec::new();
    for entry in timeline.entries {
        if matches!(entry, Entry::Event { .. }) {
            events.push(entry);
        } else {
            other.push(entry);
        }
    }
    let slots = report
        .get("slots")
        .and_then(Value::as_array)
        .context("Missing common slots")?;
    ensure!(slots.len() == events.len(), "Common slot count differs");
    let horizon = (lookahead * 1000.0).ceil() as i64;
    let longest = inputs.iter().map(|i| i.pull.end_ms).max().unwrap_or(0);
    ensure!(
        longest <= i64::MAX / 4,
        "Observed duration exceeds replay clock range"
    );
    let mut compiler = Compiler {
        inputs,
        clocks: vec![(0, 0); inputs.len()],
        origins: vec![0; inputs.len()],
        rows: Vec::new(),
        blocks: Vec::new(),
        branches: Vec::new(),
        phases: Vec::new(),
        horizon,
        // Keep labels on SPEC's tenth-second grid too, e.g. a 6051ms pull needs a rounded-up guard.
        guard: (longest + horizon + 2600 + 99) / 100 * 100,
        next: 0,
    };
    let anchors: Vec<_> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| {
            let refs = samples(slot, inputs);
            !refs.is_empty()
                && refs.iter().all(|(_, s)| s.key.role.ends_with("/Boss"))
                && slot["abilityIds"]
                    .as_array()
                    .is_some_and(|ids| ids.len() == 1)
        })
        .map(|(i, _)| i)
        .collect();
    let mut candidates = BTreeMap::new();
    let mut before = None;
    for after in anchors.into_iter().filter(|_| branches) {
        let variants = paths(
            before.and_then(|i| slots.get(i)),
            slots.get(after).context("Missing endpoint")?,
            inputs,
        )?;
        let start = before.map_or(0, |i| i + 1);
        // Keep P5's proven scalar/ID-array rows when every cast is already represented.
        let omitted = variants.iter().flatten().any(|(i, signals)| {
            signals.iter().any(|signal| {
                !slots
                    .iter()
                    .skip(start)
                    .take(after + 1 - start)
                    .any(|slot| {
                        inputs.get(*i).is_some_and(|input| {
                            sample_index(slot, &input.pull.file)
                                == signal.event_indices.first().copied()
                        })
                    })
            })
        });
        if !variants.is_empty() && omitted {
            candidates.insert(start, (after, variants));
        }
        before = Some(after);
    }
    let mut position = 0;
    let mut events = events.into_iter().map(Some).collect::<Vec<_>>();
    while position < slots.len() {
        if let Some((after, variants)) = candidates.get(&position) {
            if let Err(error) = compiler.branch(variants) {
                let mut report = report;
                put(
                    RejectedExtension {
                        accepted: false,
                        reason: format!("{error:#}"),
                        lookahead_ms: horizon,
                        checks: None,
                    },
                    &mut report,
                    "extensions",
                )?;
                return Ok((yaml, report));
            }
            position = after + 1;
            continue;
        }
        let mut entry = events
            .get_mut(position)
            .and_then(Option::take)
            .context("Missing common event")?;
        let refs = samples(slots.get(position).context("Missing common slot")?, inputs);
        if let Entry::Event {
            sync: Some(Sync::Network(sync)),
            note,
            ..
        } = &mut entry
        {
            // Reconsider drift-disabled rows only; raw collisions and unsafe source names stay disabled.
            if !sync.enabled
                && note
                    .as_ref()
                    .is_some_and(|n| n.contains("observed cast falls outside"))
            {
                sync.enabled = true;
                *note = None;
            }
        }
        let mut slot: PhaseSlot =
            serde_json::from_value(slots.get(position).context("Missing common slot")?.clone())?;
        slot.alignment_slot = Some(position);
        let occupied = compiler.emit(entry, &refs, slot, None)?;
        let row = compiler.rows.last_mut().context("Missing common row")?;
        let wide = row
            .1
            .as_ref()
            .and_then(|s| s.window_ms)
            .is_some_and(|w| w.iter().any(|&v| v > 2500));
        if wide && sync_enabled(row) {
            let label = format!("phase-{}", compiler.phases.len());
            if let Entry::Event { jump, .. } = &mut row.0 {
                *jump = Some(Jump {
                    to: Destination::Label(label.clone()),
                    when: JumpWhen::Sync,
                });
            }
            let at = compiler.allocate(occupied)?;
            compiler.label(&label, at, &refs)?;
            for (i, s) in &refs {
                *compiler.clocks.get_mut(*i).context("Missing phase clock")? = (s.time_ms, at);
                *compiler
                    .origins
                    .get_mut(*i)
                    .context("Missing phase origin")? = s.time_ms;
            }
            compiler.phases.push(Phase {
                label,
                entry_ms: at,
                alignment_slot: position,
                selector_slot: 0,
            });
        }
        position += 1;
    }
    if compiler.branches.is_empty() && compiler.phases.is_empty() {
        return Ok((yaml, report));
    }
    compiler
        .rows
        .sort_by(|a, b| time(&a.0).total_cmp(&time(&b.0)));
    let mut compiled_slots = Vec::new();
    let mut conflicts = Vec::new();
    let mut entries = vec![Entry::Note {
        text: format!(
            "Draft with discriminated paths and observed phase windows; independent lookahead horizon {lookahead}s. No forcejump; runtime display unverified."
        ),
    }];
    let mut block = 0;
    compiler.blocks.sort_by_key(|b| b.draft_entry_ms);
    for (mut entry, slot) in compiler.rows {
        if let Some(mut slot) = slot {
            slot.id = compiled_slots.len();
            slot.block = block;
            if let Entry::Event {
                sync: Some(Sync::Network(sync)),
                note,
                ..
            } = &mut entry
                && !sync.enabled
            {
                for conflict in report
                    .get("syncConflicts")
                    .context("Missing original conflicts")?
                    .as_array()
                    .context("Missing original conflicts")?
                    .iter()
                    .filter(|conflict| {
                        conflict.get("slot").and_then(Value::as_u64)
                            == slot.alignment_slot.map(|i| i as u64)
                    })
                {
                    let mut conflict = conflict.clone();
                    put(compiled_slots.len(), &mut conflict, "slot")?;
                    *note = Some(format!(
                        "Sync disabled: {}; see slot {} in report",
                        conflict
                            .get("reason")
                            .context("Missing conflict reason")?
                            .as_str()
                            .context("Missing conflict reason")?,
                        compiled_slots.len()
                    ));
                    conflicts.push(conflict);
                }
            }
            compiled_slots.push(slot);
        } else {
            block += 1;
        }
        entries.push(entry);
    }
    for b in &mut compiler.branches {
        for path in &mut b.paths {
            let matching: Vec<_> = compiled_slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.path.as_deref() == Some(path.label.as_str()))
                .collect();
            path.selector_slot = matching
                .iter()
                .find(|(_, s)| s.selector == Some(true))
                .context("Missing selector")?
                .0;
            path.slots = matching.iter().map(|(i, _)| *i).collect();
        }
    }
    for phase in &mut compiler.phases {
        phase.selector_slot = compiled_slots
            .iter()
            .position(|slot| slot.alignment_slot == Some(phase.alignment_slot))
            .context("Missing phase selector")?;
    }
    entries.extend(
        other
            .into_iter()
            .filter(|entry| matches!(entry, Entry::AbilityCatalog { .. })),
    );
    let compiled_yaml = match draft::serialize_draft(entries) {
        Ok(yaml) => yaml,
        Err(error) => {
            let mut report = report;
            put(
                RejectedExtension {
                    accepted: false,
                    lookahead_ms: horizon,
                    reason: format!("{error:#}"),
                    checks: None,
                },
                &mut report,
                "extensions",
            )?;
            return Ok((yaml, report));
        }
    };
    let mut compiled_report = report.clone();
    put(&compiled_slots, &mut compiled_report, "slots")?;
    put(conflicts, &mut compiled_report, "syncConflicts")?;
    put(
        report
            .get("slots")
            .context("Missing alignment slots")?
            .clone(),
        &mut compiled_report,
        "alignmentSlots",
    )?;
    put(
        compiled_report
            .get("blocks")
            .context("Missing alignment blocks")?
            .clone(),
        &mut compiled_report,
        "alignmentBlocks",
    )?;
    let mut blocks = vec![PhaseBlock {
        id: 0,
        entry: "fightStart",
        label: None,
        draft_entry_ms: 0,
        time: statistics(vec![0; inputs.len()])?,
    }];
    for (i, mut b) in compiler.blocks.into_iter().enumerate() {
        b.id = i + 1;
        blocks.push(b);
    }
    put(blocks, &mut compiled_report, "blocks")?;
    let mut extension = AcceptedExtension {
        accepted: true,
        lookahead_ms: compiler.horizon,
        max_observed_end_ms: longest,
        branches: compiler.branches,
        phases: compiler.phases,
        forcejump_generated: false,
        preview_projection_executed: true,
        actual_display_executed: false,
        preview: "independent projection; actual runtime display not executed",
        checks: Vec::new(),
    };
    put(&extension, &mut compiled_report, "extensions")?;
    let peers = inputs
        .iter()
        .map(|input| (input.pull.file.as_str(), &input.pull))
        .collect();
    let key = serde_json::from_value::<BTreeMap<String, i64>>(
        report
            .get("group")
            .context("Missing generation group")?
            .clone(),
    )?;
    let group = super::GroupKey {
        encounter: *key.get("encounter").context("Missing encounter")?,
        difficulty: *key.get("difficulty").context("Missing difficulty")?,
    };
    let mut checks = Vec::new();
    for input in inputs {
        let evidence = replay::evidence(
            &compiled_yaml,
            &compiled_report,
            &group,
            &input.pull,
            &input.log,
            &peers,
        )?;
        let result = crate::timeline::replay::run(
            &compiled_yaml,
            &replay::signals(&input.log)?,
            input.pull.end_ms,
            &evidence,
        )?;
        checks.push(ReplayCheck {
            file: input.pull.file.clone(),
            passed: result.passed,
            summary: result.summary,
            jumps: result.jumps,
            previews: result.previews,
        });
    }
    if checks.iter().any(|check| !check.passed) {
        let mut report = report;
        put(
            RejectedExtension {
                accepted: false,
                lookahead_ms: horizon,
                reason: "candidate failed independent raw-signal replay".into(),
                checks: Some(checks),
            },
            &mut report,
            "extensions",
        )?;
        return Ok((yaml, report));
    }
    extension.checks = checks;
    put(extension, &mut compiled_report, "extensions")?;
    put(
        true,
        compiled_report
            .get_mut("validation")
            .context("Missing validation")?,
        "replay",
    )?;
    put(
        [
            "Only observed discriminated paths are compiled; unknown paths require holdout replay.",
            "Virtual isolation covers observed pull durations and the configured independent lookahead horizon.",
            "Repeats remain finite; forcejump and cross-encounter connections are not generated.",
            "cactbot parser, actual runtime display, priority and reset compatibility remain unverified.",
        ],
        &mut compiled_report,
        "limitations",
    )?;
    for coverage in compiled_report
        .get_mut("outputCoverage")
        .context("Missing coverage")?
        .as_array_mut()
        .context("Missing coverage")?
    {
        let represented: BTreeSet<_> = compiled_slots
            .iter()
            .flat_map(|slot| &slot.samples)
            .filter(|s| coverage["file"] == s.file)
            .flat_map(|s| s.event_indices.iter().copied())
            .collect();
        put(&represented, coverage, "representedEventIndices")?;
        let input = inputs
            .iter()
            .find(|input| input.pull.file == coverage["file"])
            .context("Missing coverage input")?;
        put(
            input
                .signals
                .values()
                .filter(|s| s.key.kind == "cast")
                .flat_map(|s| &s.event_indices)
                .filter(|&&i| !represented.contains(&i))
                .collect::<Vec<_>>(),
            coverage,
            "omittedEventIndices",
        )?;
    }
    let coverage = compiled_report
        .get("outputCoverage")
        .context("Missing coverage")?
        .clone();
    compiled_report
        .get_mut("omittedSignals")
        .context("Missing omitted signals")?
        .as_array_mut()
        .context("Missing omitted signals")?
        .retain(|signal| {
            !coverage.as_array().is_some_and(|items| {
                items.iter().any(|item| {
                    item["file"] == signal["file"]
                        && signal["eventIndices"].as_array().is_some_and(|indices| {
                            indices.iter().all(|index| {
                                item["representedEventIndices"]
                                    .as_array()
                                    .is_some_and(|represented| represented.contains(index))
                            })
                        })
                })
            })
        });
    Ok((compiled_yaml, compiled_report))
}

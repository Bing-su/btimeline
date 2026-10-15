use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use itertools::Itertools;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::alignment::{Signal, SignalKey, match_key as path_key};
use super::multi::{Input, field_pattern, statistics};
use super::report::TimeStatistics;
use super::{draft, replay};
use crate::timeline::{Destination, Entry, Jump, JumpWhen, LogType, NetworkSync, Sync, Timeline};

type Samples<'a> = Vec<(usize, &'a Signal)>;
type Paths<'a> = Vec<Vec<(usize, Vec<&'a Signal>)>>;

// Copies can reverse their packet order, so retain starts from every member of the merged cast.
fn started_signals(inputs: &[Input]) -> BTreeSet<SignalKey> {
    inputs
        .iter()
        .flat_map(|input| {
            input
                .signals
                .values()
                .filter(|signal| {
                    signal.key.kind == "cast"
                        && input.pull.occurrences.iter().any(|row| {
                            signal.event_indices.contains(&row.event_index)
                                && row.start_event_index.is_some()
                        })
                })
                .map(path_key)
        })
        .collect()
}

// Unstarted fan-out effects are references, e.g. a final Stray Apocalypse packet cannot dispatch a path.
fn instant_helpers(inputs: &[Input]) -> BTreeSet<SignalKey> {
    let fanout: BTreeSet<_> = inputs
        .iter()
        .flat_map(|input| input.signals.values())
        .filter(|signal| signal.key.instances > 1)
        .map(|signal| {
            (
                signal.key.actor_game_id,
                &signal.key.actor_name,
                &signal.key.role,
            )
        })
        .collect();
    let started = started_signals(inputs);
    let mut clears = inputs.iter().filter(|input| input.pull.kill).map(|input| {
        input
            .signals
            .values()
            .filter(|signal| signal.key.kind == "cast")
            .map(path_key)
            .collect::<BTreeSet<_>>()
    });
    let mut common = clears.next().unwrap_or_default();
    for casts in clears {
        common.retain(|key| casts.contains(key));
    }
    let mut rapid_followups = BTreeSet::new();
    for input in inputs {
        let mut times: BTreeMap<SignalKey, Vec<i64>> = BTreeMap::new();
        for signal in input
            .signals
            .values()
            .filter(|signal| signal.key.kind == "cast" && signal.key.role.ends_with("/Boss"))
        {
            times
                .entry(path_key(signal))
                .or_default()
                .push(signal.time_ms);
        }
        for (key, mut times) in times {
            times.sort();
            if common.contains(&key) && times.iter().tuple_windows().any(|(a, b)| b - a <= 2500) {
                // Target-dependent follow-ups have no own start, e.g. Hyperdrive stops when its tank dies.
                rapid_followups.insert(key);
            }
        }
    }
    inputs
        .iter()
        .flat_map(|input| input.signals.values())
        .filter(|signal| {
            signal.key.kind == "cast"
                && ((!signal.key.role.ends_with("/Boss")
                    && (signal.key.actor_name.is_some()
                        || fanout.contains(&(
                            signal.key.actor_game_id,
                            &signal.key.actor_name,
                            &signal.key.role,
                        ))))
                    || rapid_followups.contains(&path_key(signal)))
                && !started.contains(&path_key(signal))
        })
        .map(path_key)
        .collect()
}

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
    #[serde(skip)]
    ancestors: Vec<String>,
    #[serde(skip)]
    origins: Vec<i64>,
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
    packet_wave: bool,
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
pub(super) struct ReplayCheck {
    pub file: String,
    pub passed: bool,
    pub summary: crate::timeline::replay::Summary,
    pub jumps: Vec<crate::timeline::replay::JumpTrace>,
    pub previews: Vec<crate::timeline::replay::Preview>,
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
                .filter_map(|(_, casts)| casts.get(position).map(|s| path_key(s)))
                .collect();
            keys.len() == 1
        })
        .count()
}

// Keep variant grouping when advancing past emitted rows, e.g. X→P/Q remains two suffixes.
fn path_suffixes<'a>(paths: &Paths<'a>, start: usize) -> Result<Paths<'a>> {
    paths
        .iter()
        .map(|path| {
            path.iter()
                .map(|(i, casts)| {
                    casts
                        .get(start..)
                        .map(|suffix| (*i, suffix.to_vec()))
                        .context("Invalid path suffix")
                })
                .collect::<Result<_>>()
        })
        .collect()
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
fn paths<'a>(
    before: Option<&Value>,
    after: &Value,
    inputs: &'a [Input],
    boss_clocks: bool,
    helpers: &BTreeSet<SignalKey>,
    reactive: &mut BTreeSet<(usize, usize)>,
) -> Result<Paths<'a>> {
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
        if boss_clocks {
            let endpoint = input
                .signals
                .get(&end)
                .context("Missing boss path endpoint")?;
            let &(lo, hi) = input
                .pull
                .boss_spans
                .iter()
                .find(|&&(lo, hi)| lo <= endpoint.time_ms && endpoint.time_ms <= hi)
                .context("Missing boss path span")?;
            // Keep the opening path but exclude earlier bosses, e.g. X→P→C starts after travel.
            ordered.retain(|signal| lo <= signal.time_ms && signal.time_ms <= hi);
        }
        ordered.sort_by_key(|s| {
            (
                super::alignment::signal_order(s),
                s.event_indices.first().copied(),
            )
        });
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
        if hi < lo
            && start
                .and_then(|index| input.signals.get(&index))
                .zip(input.signals.get(&end))
                .is_some_and(|(a, b)| a.time_ms.abs_diff(b.time_ms) <= 250)
        {
            // A packet wave has no interior path, e.g. two Exdeath copies complete in opposite 45ms orders.
            return Ok(Vec::new());
        }
        let signals = ordered.get(lo..=hi).with_context(|| {
            format!(
                "Invalid branch interval: {} start {:?} end {} (positions {lo}..={hi})",
                input.pull.file, start, end
            )
        })?;
        // Packet order cannot select a path, e.g. simultaneous Spelldriver/Spellscatter arrivals swap.
        // Compare named mechanics within a wave, e.g. true/fake Blizzard IDs cannot reorder Blizzard and Thunder.
        let casts: Vec<_> = signals
            .iter()
            .filter(|s| s.key.kind == "cast")
            .copied()
            .sorted_by_key(|signal| {
                let name = input
                    .log
                    .report
                    .master_data
                    .abilities
                    .iter()
                    .find(|ability| ability.game_id == signal.key.ability_id)
                    .map(|ability| &ability.name);
                (
                    signal.wave_ms,
                    !signal.key.role.ends_with("/Boss"),
                    signal.key.actor_game_id,
                    &signal.key.actor_name,
                    &signal.key.role,
                    name,
                    signal.key.ability_id,
                )
            })
            .collect();
        // Dispatch follows completed mechanics, e.g. differing packet counts do not create extra choices.
        let keys = casts.iter().map(|s| path_key(s)).collect();
        variants.entry(keys).or_default().push((i, casts));
    }
    let mut result: Paths<'a> = variants.into_values().collect();
    {
        let fanout: BTreeSet<_> = inputs
            .iter()
            .flat_map(|input| input.signals.values())
            .filter(|signal| signal.key.instances > 1)
            .map(|signal| {
                (
                    signal.key.actor_game_id,
                    &signal.key.actor_name,
                    &signal.key.role,
                )
            })
            .collect();
        let started = started_signals(inputs);
        // Conditional instant reactions are observations, e.g. Launch cannot dispatch a scripted missile path.
        let mut optional: BTreeSet<_> = result
            .iter()
            .flatten()
            .flat_map(|(_, casts)| casts.iter())
            .filter(|signal| {
                let context = (
                    signal.key.actor_game_id,
                    &signal.key.actor_name,
                    &signal.key.role,
                );
                !signal.key.role.ends_with("/Boss")
                    && (signal.key.actor_name.is_some() || fanout.contains(&context))
                    && !started.contains(&path_key(signal))
                    && (fanout.contains(&context)
                        || result.iter().flatten().any(|(_, casts)| {
                            !casts
                                .iter()
                                .any(|other| path_key(other) == path_key(signal))
                        }))
            })
            .map(|signal| path_key(signal))
            .collect();
        optional.extend(
            helpers
                .iter()
                .filter(|key| key.role.ends_with("/Boss"))
                .cloned(),
        );
        let mut grouped = BTreeMap::new();
        for (i, mut casts) in result.into_iter().flatten() {
            casts.retain(|signal| {
                if optional.contains(&path_key(signal)) {
                    if let Some(&index) = signal.event_indices.first() {
                        reactive.insert((i, index));
                    }
                    false
                } else {
                    true
                }
            });
            grouped
                .entry(casts.iter().map(|s| path_key(s)).collect::<Vec<_>>())
                .or_insert_with(Vec::new)
                .push((i, casts));
        }
        result = grouped.into_values().collect();
    }
    if result.is_empty()
        || result
            .iter()
            .any(|path| path.iter().any(|(_, casts)| casts.len() < 2))
    {
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
        // The consumer excludes the upper edge, e.g. an observation at +5s needs window after 5.1s.
        let after = (((timing.max_ms - at).max(0) / 100 + 1) * 100).max(2500);
        let Entry::Event {
            at: entry_at,
            sync,
            jump,
            note,
            ..
        } = &mut entry
        else {
            anyhow::bail!("Missing phase event")
        };
        *entry_at = at as f64 / 1000.0;
        let Some(Sync::Network(sync)) = sync else {
            anyhow::bail!("Missing phase sync")
        };
        if target.is_none()
            && self
                .blocks
                .last()
                .and_then(|block| block.label.as_deref())
                .is_some_and(|label| label.starts_with("branch-") && !label.ends_with("-merge"))
            && samples.iter().any(|(i, signal)| {
                self.origins
                    .get(*i)
                    .is_some_and(|&origin| origin > 0 && signal.time_ms.abs_diff(origin) <= 100)
            })
        {
            // One fan-out can arrive before its selector, e.g. Blizzard packets precede the same-tick boss jump.
            sync.enabled = false;
            *note = Some("Sync disabled: simultaneous cast wave uses its observed timing without another clock correction.".into());
        }
        sync.window = Some([before as f64 / 1000.0, after as f64 / 1000.0]);
        if let Some(target) = target {
            ensure!(
                sync.enabled,
                "Branch needs an enabled discriminator and merge sync: {:?} at {:?}",
                sync.fields,
                samples
                    .iter()
                    .map(|(i, signal)| (*i, signal.time_ms))
                    .collect::<Vec<_>>()
            );
            *jump = Some(Jump {
                to: Destination::Label(target.into()),
                when: JumpWhen::Sync,
            });
        }
        slot.at_ms = at.into();
        slot.origins.clone_from(&self.origins);
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
        let mut ability_names = Vec::new();
        for (i, signal) in samples {
            let input = self.inputs.get(*i).context("Missing branch input")?;
            ids.extend(signal.event_indices.iter().filter_map(|&index| {
                input
                    .log
                    .events
                    .get(index)
                    .and_then(|event| event.ability_game_id)
            }));
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
            ability_names.push(
                input
                    .log
                    .report
                    .master_data
                    .abilities
                    .iter()
                    .find(|a| a.game_id == signal.key.ability_id)
                    .map(|a| a.name.as_str())
                    .context("Missing branch ability")?,
            );
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
                field_pattern(ids.iter().map(|id| format!("{id:X}")).collect())?,
            ),
            (
                "source".into(),
                field_pattern(names.iter().map(|s| regress::escape(s)).collect())?,
            ),
        ]);
        let times = statistics(samples.iter().map(|(_, s)| s.time_ms).collect())?;
        // A main cast and its nearby helper can share the wire signature, e.g. All Things Ending.
        let collision = samples.iter().any(|(i, signal)| {
            signal.key.kind == "cast"
                && self.inputs.get(*i).is_some_and(|input| {
                    input.pull.occurrences.iter().any(|row| {
                        row.kind == signal.key.kind
                            && ids.contains(&row.ability_id)
                            && input
                                .actors
                                .get(&row.actor_id)
                                .is_some_and(|name| names.contains(name))
                            && !signal.event_indices.contains(&row.event_index)
                            && row.relative_ms.abs_diff(signal.time_ms) <= 500
                    })
                })
        });
        Ok((
            Entry::Event {
                at: 0.0,
                name: ability_names.into_iter().unique().join(" / "),
                duration: None,
                sync: Some(Sync::Network(NetworkSync {
                    log: if samples
                        .iter()
                        .all(|(_, signal)| signal.key.kind == "begincast")
                    {
                        LogType::StartsUsing
                    } else {
                        LogType::Ability
                    },
                    fields,
                    enabled: !collision,
                    window: None,
                })),
                jump: None,
                note: collision.then(|| {
                    "Sync disabled: nearby cast shares the same source and ability.".into()
                }),
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
                ancestors: Vec::new(),
                origins: Vec::new(),
            },
        ))
    }

    // Share common-row evidence and opening windows, e.g. a sole merged choice has no jump.
    fn emit_common(
        &mut self,
        paths: &Paths<'a>,
        position: usize,
        opening: bool,
        sync_safe: bool,
    ) -> Result<()> {
        let refs: Samples<'_> = paths
            .iter()
            .flatten()
            .filter_map(|(i, casts)| Some((*i, *casts.get(position)?)))
            .collect();
        let (mut entry, mut slot) = self.branch_entry(&refs)?;
        slot.evidence = "observedCommonPrefix".into();
        if !sync_safe
            && let Entry::Event {
                sync: Some(Sync::Network(sync)),
                note,
                ..
            } = &mut entry
        {
            // A shared effect may arrive after dispatch, e.g. Blizzard packets follow their same-tick selector.
            sync.enabled = false;
            *note =
                Some("Sync disabled: common effect shares the discriminator's packet wave.".into());
        }
        self.emit(entry, &refs, slot, None)?;
        if opening {
            open_entry(self.rows.last_mut().context("Missing opening row")?);
        }
        Ok(())
    }

    // Group by selector first so differing tails still dispatch together, e.g. X→P/Q versus Y→R.
    fn group_paths(&self, variants: Paths<'a>) -> Result<Vec<Paths<'a>>> {
        let mut groups: BTreeMap<SignalKey, Paths<'_>> = BTreeMap::new();
        for path in variants {
            let (i, casts) = path.first().context("Missing path choice")?;
            let signal = casts.first().context("Missing path choice")?;
            let input = self.inputs.get(*i).context("Missing choice input")?;
            let source = signal
                .event_indices
                .first()
                .and_then(|&index| input.log.events.get(index))
                .and_then(|event| event.source_id)
                .and_then(|id| input.actors.get(&id))
                .context("Missing choice source")?;
            // Only the wire signature can dispatch, e.g. Kefka's boss and helper copies cannot be sibling choices.
            let mut key = path_key(signal);
            key.actor_game_id = 0;
            key.actor_name = Some(source.clone());
            key.role.clear();
            groups.entry(key).or_default().push(path);
        }
        let mut merged: Vec<Paths<'_>> = Vec::new();
        // Different choices can share a continuation, e.g. Halo/Miasma both lead to Puppet Strings.
        let mut equivalent: BTreeMap<_, usize> = BTreeMap::new();
        for paths in groups.into_values() {
            let keys = paths
                .iter()
                .map(|path| {
                    let (i, casts) = path.first().context("Missing equivalent samples")?;
                    let input = self.inputs.get(*i).context("Missing equivalent input")?;
                    casts
                        .iter()
                        .enumerate()
                        .map(|(position, signal)| {
                            let name = input
                                .log
                                .report
                                .master_data
                                .abilities
                                .iter()
                                .find(|ability| ability.game_id == signal.key.ability_id)
                                .map(|ability| ability.name.clone())
                                .context("Missing path ability")?;
                            let mut key = path_key(signal);
                            key.ability_id = 0;
                            Ok((key, (position > 0).then_some(name)))
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<BTreeSet<_>>>()?;
            // A selector may cover several ID combinations, e.g. BEE4→BEE9/BEEB has one named path.
            if keys.len() != 1 {
                merged.push(paths);
                continue;
            }
            let key = keys
                .into_iter()
                .next()
                .context("Missing equivalent path key")?;
            let mut samples = paths.into_iter().flatten().collect();
            if let Some(&prior) = equivalent.get(&key) {
                let target = merged
                    .get_mut(prior)
                    .and_then(|paths| paths.first_mut())
                    .context("Missing merged path")?;
                target.append(&mut samples);
            } else {
                equivalent.insert(key, merged.len());
                merged.push(vec![samples]);
            }
        }
        Ok(merged)
    }

    fn branch(&mut self, variants: &Paths<'a>, opening: bool) -> Result<()> {
        let common = common_prefix(variants);
        if variants
            .iter()
            .flatten()
            .all(|(_, casts)| casts.len() == common)
        {
            for position in 0..common {
                self.emit_common(variants, position, opening && position == 0, true)?;
            }
            return Ok(());
        }
        let prefix = (0..common)
            .take_while(|&position| {
                variants.iter().flatten().all(|(i, casts)| {
                    casts
                        .get(position)
                        .zip(casts.get(common))
                        .is_some_and(|(a, b)| {
                            // A later discriminator can start during this prefix, e.g. All Things Ending overlaps the first beam.
                            let start = casts
                                .iter()
                                .skip(common)
                                .flat_map(|signal| {
                                    self.inputs.get(*i).into_iter().flat_map(move |input| {
                                        input
                                            .pull
                                            .occurrences
                                            .iter()
                                            .filter(move |row| {
                                                signal.event_indices.contains(&row.event_index)
                                            })
                                            .filter_map(|row| {
                                                row.start_timestamp_ms.map(|start| {
                                                    start - row.timestamp_ms + row.relative_ms
                                                })
                                            })
                                    })
                                })
                                .min()
                                .unwrap_or(b.time_ms);
                            a.wave_ms < b.wave_ms && a.time_ms < start
                        })
                })
            })
            .count();
        for position in 0..prefix {
            self.emit_common(variants, position, opening && position == 0, true)?;
        }
        let shortened = path_suffixes(variants, prefix)?;
        let variants = &shortened;
        let opening = opening && prefix == 0;
        let mut orders: BTreeMap<Vec<SignalKey>, Vec<(usize, Vec<&Signal>)>> = BTreeMap::new();
        for (i, casts) in variants.iter().flatten() {
            let count = casts.first().map_or(0, |first| {
                casts
                    .iter()
                    .take_while(|signal| signal.wave_ms == first.wave_ms)
                    .count()
            });
            // Compile each possible leading packet; recursion consumes the remainder without inventing combinations.
            for choice in 0..count.max(1) {
                let mut order = casts.clone();
                if choice > 0 {
                    let signal = order.remove(choice);
                    order.insert(0, signal);
                }
                orders
                    .entry(order.iter().map(|signal| path_key(signal)).collect())
                    .or_default()
                    .push((*i, order));
            }
        }
        let variants: Paths<'_> = orders.into_values().collect();
        let variants = &variants;
        let prefix = common_prefix(variants);
        if variants
            .iter()
            .flatten()
            .all(|(_, casts)| casts.len() == prefix)
        {
            for position in 0..prefix {
                self.emit_common(variants, position, opening && position == 0, true)?;
            }
            return Ok(());
        }
        let group = self.branches.len();
        let merge_label = format!("branch-{group}-merge");
        let mut groups = self.group_paths(path_suffixes(variants, prefix)?)?;
        let mut observed = BTreeSet::new();
        let packet_wave = groups
            .iter()
            .flatten()
            .flatten()
            .any(|(i, _)| !observed.insert(*i));
        let mut selector_samples: Vec<Samples<'_>> = groups
            .iter()
            .map(|paths| {
                paths
                    .iter()
                    .flatten()
                    .filter_map(|(i, casts)| Some((*i, *casts.first()?)))
                    .map(|(i, signal)| ((i, signal.event_indices.clone()), (i, signal)))
                    .collect::<BTreeMap<_, _>>()
                    .into_values()
                    .collect()
            })
            .collect();
        let starts = groups.len() > 1 && selector_samples.iter().map(|refs| self.branch_entry(refs))
            .collect::<Result<Vec<_>>>()?.iter().any(|(entry, _)| {
                matches!(entry, Entry::Event { sync: Some(Sync::Network(sync)), .. } if !sync.enabled)
            });
        if starts {
            // Dispatch before the effects arrive, e.g. Gravity III's main cast shares its helper's Ability signature.
            for refs in &mut selector_samples {
                for (i, signal) in refs {
                    let input = self.inputs.get(*i).context("Missing selector input")?;
                    let start = input
                        .pull
                        .occurrences
                        .iter()
                        .filter(|row| signal.event_indices.contains(&row.event_index))
                        .filter_map(|row| row.start_event_index.zip(row.start_timestamp_ms))
                        .min_by_key(|&(index, at)| (at, index))
                        .map(|(index, _)| index)
                        .context("Ambiguous discriminator has no cast start")?;
                    *signal = input
                        .signals
                        .values()
                        .find(|signal| signal.event_indices.contains(&start))
                        .context("Missing discriminator start")?;
                }
            }
        }
        let preserved_prefix = if starts {
            // Keep effects after the start inside the selected path, e.g. Mystery Magic completes with Blizzard.
            (0..prefix)
                .take_while(|&position| {
                    variants.iter().flatten().all(|(i, casts)| {
                        casts
                            .get(position)
                            .zip(
                                selector_samples
                                    .iter()
                                    .flatten()
                                    .filter(|(other, _)| other == i)
                                    .map(|(_, start)| start.time_ms)
                                    .min(),
                            )
                            .is_some_and(|(cast, start)| cast.time_ms < start)
                    })
                })
                .count()
        } else {
            prefix
        };
        for position in 0..preserved_prefix {
            let shares_dispatch = variants.iter().flatten().any(|(_, casts)| {
                casts
                    .get(position)
                    .zip(casts.get(prefix))
                    .is_some_and(|(common, choice)| common.time_ms.abs_diff(choice.time_ms) <= 250)
            });
            self.emit_common(
                variants,
                position,
                opening && position == 0,
                !shares_dispatch,
            )?;
        }
        // A sole choice needs no jump, e.g. merged C3FF/C400 continues on the current clock.
        if let [paths] = groups.as_slice() {
            self.emit_common(paths, 0, opening && prefix == 0, true)?;
            let suffix = path_suffixes(paths, 1)?;
            return self.branch(&suffix, false);
        }
        if starts {
            for (i, casts) in groups.iter_mut().flatten().flatten() {
                let full = variants
                    .iter()
                    .flatten()
                    .find(|(other, full)| {
                        other == i
                            && full.get(prefix..).is_some_and(|tail| {
                                tail.len() == casts.len()
                                    && tail
                                        .iter()
                                        .zip(casts.iter())
                                        .all(|(a, b)| a.event_indices == b.event_indices)
                            })
                    })
                    .context("Missing full discriminator path")?;
                *casts = full
                    .1
                    .get(preserved_prefix..)
                    .context("Invalid prefix boundary")?
                    .to_vec();
            }
        }
        let original_clocks = self.clocks.clone();
        let arrivals = selector_samples
            .iter()
            .flatten()
            .map(|(i, signal)| {
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
        for (path_id, refs) in selector_samples.into_iter().enumerate() {
            self.clocks.clone_from(&original_clocks);
            let (mut entry, mut slot) = self.branch_entry(&refs)?;
            if starts && let Entry::Event { name, .. } = &mut entry {
                *name = "--sync--".into();
            }
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
                // Open sibling choices from zero only at entry, e.g. nested P/Q still needs X first.
                let before = if opening && prefix == 0 {
                    at_ms
                } else {
                    before
                };
                let after = (((envelope.max_ms - at_ms).max(0) / 100 + 1) * 100).max(2500);
                sync.window = Some([before as f64 / 1000.0, after as f64 / 1000.0]);
                let slot = slot.as_mut().context("Missing selector slot")?;
                slot.window_ms = Some([before, after]);
                slot.transition_clock_range = Some(envelope);
                occupied = occupied.max(at_ms + after);
            }
            selectors.push((label, refs));
        }
        // Reserve the parent before recursing, e.g. X→P/Q and Y→R need distinct branch labels.
        self.branches.push(Branch {
            id: group,
            packet_wave,
            merge_label: merge_label.clone(),
            merge_ms: 0,
            paths: Vec::new(),
        });
        let mut path_reports = Vec::new();
        let mut merges = Vec::new();
        for (path_id, paths) in groups.iter().enumerate() {
            let path: Vec<_> = paths.iter().flatten().collect();
            let (label, refs) = selectors.get(path_id).context("Missing branch selector")?;
            let base = self.allocate(occupied)?;
            self.label(label, base, refs)?;
            for (i, s) in refs {
                *self.clocks.get_mut(*i).context("Missing branch clock")? = (s.time_ms, base);
                *self.origins.get_mut(*i).context("Missing branch origin")? = s.time_ms;
            }
            let row_start = self.rows.len();
            let child_start = self.branches.len();
            if paths.len() > 1 {
                let suffix = path_suffixes(paths, usize::from(!starts))?;
                self.branch(&suffix, false)?;
                occupied = occupied.max(self.next);
                let endpoints: BTreeSet<_> = suffix
                    .iter()
                    .flatten()
                    .filter_map(|(i, casts)| Some((*i, *casts.last()?.event_indices.first()?)))
                    .collect();
                for (entry, slot) in self.rows.iter_mut().skip(row_start) {
                    if let Some(slot) = slot {
                        slot.ancestors.push(label.clone());
                        if slot.samples.iter().any(|sample| {
                            self.inputs
                                .iter()
                                .position(|input| input.pull.file == sample.file)
                                .is_some_and(|i| {
                                    sample
                                        .event_indices
                                        .first()
                                        .is_some_and(|index| endpoints.contains(&(i, *index)))
                                })
                        }) && let Entry::Event { jump, .. } = entry
                        {
                            *jump = Some(Jump {
                                to: Destination::Label(merge_label.clone()),
                                when: JumpWhen::Sync,
                            });
                        }
                    }
                }
                for child in self.branches.iter_mut().skip(child_start) {
                    child.merge_label.clone_from(&merge_label);
                    for path in &mut child.paths {
                        path.merge_label.clone_from(&merge_label);
                    }
                }
                merges.extend(
                    suffix
                        .iter()
                        .flatten()
                        .filter_map(|(i, casts)| Some((*i, *casts.last()?))),
                );
            } else {
                let length = path.first().context("Empty branch")?.1.len();
                if length == 1 && !starts {
                    for (entry, slot) in &mut self.rows {
                        if slot.as_ref().is_some_and(|slot| {
                            slot.path.as_deref() == Some(label.as_str())
                                && slot.selector == Some(true)
                        }) && let Entry::Event { jump, .. } = entry
                        {
                            *jump = Some(Jump {
                                to: Destination::Label(merge_label.clone()),
                                when: JumpWhen::Sync,
                            });
                        }
                    }
                    merges.extend(refs.iter().copied());
                }
                for position in usize::from(!starts)..length {
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
        for child in self
            .branches
            .iter_mut()
            .skip(group + 1)
            .filter(|child| child.merge_label == merge_label)
        {
            child.merge_ms = merge_at;
        }
        *self
            .branches
            .get_mut(group)
            .context("Missing parent branch")? = Branch {
            id: group,
            packet_wave,
            merge_label,
            merge_ms: merge_at,
            paths: path_reports,
        };
        Ok(())
    }
}

fn sync_enabled(row: &(Entry, Option<PhaseSlot>)) -> bool {
    matches!(&row.0, Entry::Event { sync:Some(Sync::Network(sync)), .. } if sync.enabled)
}

// A seal reset stops at zero, e.g. either opening selector must activate before travel advances time.
fn open_entry(row: &mut (Entry, Option<PhaseSlot>)) {
    if let (
        Entry::Event {
            at,
            sync: Some(Sync::Network(sync)),
            ..
        },
        Some(slot),
    ) = row
    {
        let mut window = sync.window.unwrap_or([2.5, 2.5]);
        window[0] = *at;
        sync.window = Some(window);
        slot.window_ms = Some(window.map(|value| (value * 1000.0).round() as i64));
    }
}

pub(super) fn expand(
    yaml: String,
    report: Value,
    inputs: &[Input],
    lookahead: f64,
) -> Result<(String, Value)> {
    let mut disabled = BTreeSet::new();
    let mut rejected_phases = BTreeSet::new();
    let (mut yaml, mut report) = compile(
        yaml,
        report,
        inputs,
        lookahead,
        true,
        &disabled,
        &mut rejected_phases,
    )?;
    let original_rejection = report.get("extensions").cloned();
    while report
        .pointer("/extensions/accepted")
        .and_then(Value::as_bool)
        == Some(false)
        && rejected_phases
            .iter()
            .any(|phase| !disabled.contains(phase))
    {
        // Keep independently verified phases, e.g. an unseen enrage must not erase earlier color choices.
        disabled.extend(std::mem::take(&mut rejected_phases));
        (yaml, report) = compile(
            yaml,
            report,
            inputs,
            lookahead,
            true,
            &disabled,
            &mut rejected_phases,
        )?;
    }
    if !disabled.is_empty()
        && report
            .pointer("/extensions/accepted")
            .and_then(Value::as_bool)
            == Some(true)
    {
        let extension = report
            .get_mut("extensions")
            .context("Missing phase evidence")?;
        put(&disabled, extension, "unresolvedPhaseIds")?;
        put(
            "Unresolved phases retain observed reference rows; their path synchronization is not inferred.",
            extension,
            "unresolvedPhasePolicy",
        )?;
        if let Some(rejected) = original_rejection {
            put(rejected, extension, "rejectedBranchCandidate")?;
        }
        return Ok((yaml, report));
    }
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
        let (phase_yaml, mut phase_report) = compile(
            yaml.clone(),
            report.clone(),
            inputs,
            lookahead,
            false,
            &disabled,
            &mut rejected_phases,
        )?;
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
    disabled: &BTreeSet<i64>,
    rejected_phases: &mut BTreeSet<i64>,
) -> Result<(String, Value)> {
    let timeline: Timeline = serde_saphyr::from_str(&yaml)?;
    let mut events = Vec::new();
    let mut other = Vec::new();
    for entry in timeline.entries {
        if entry.is_combat_start() {
            continue;
        }
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
    let reports: Vec<_> = report
        .get("inputs")
        .and_then(Value::as_array)
        .context("Missing phase inputs")?
        .iter()
        .collect();
    let raid = report.get("mode").and_then(Value::as_str) == Some("raid");
    let boss_clocks = !raid
        && reports
            .iter()
            .any(|input| super::sections::spans(input).is_ok_and(|spans| spans.len() > 1));
    let sections = slots
        .iter()
        .map(|slot| {
            if boss_clocks {
                super::sections::slot_section(slot, &reports, false)
            } else {
                Ok(0)
            }
        })
        .collect::<Result<Vec<_>>>()?;
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
                && refs.iter().all(|(i, signal)| {
                    inputs.get(*i).is_some_and(|input| {
                        !input.signals.values().any(|other| {
                            other.key.kind == "cast" && other.wave_ms == signal.wave_ms
                                && other.event_indices.first() != signal.event_indices.first()
                        })
                    })
                })
        })
        .filter(|(i, _)| {
            // An ambiguous common cast cannot merge paths, e.g. the helper completes before the boss copy.
            !matches!(events.get(*i), Some(Entry::Event { sync: Some(Sync::Network(sync)), note, .. })
                if !sync.enabled && !note.as_ref().is_some_and(|note| note.contains("observed cast falls outside")))
        })
        .map(|(i, _)| i)
        .collect();
    let mut candidates = BTreeMap::new();
    // Preserve all casts in the final fallback, e.g. a rejected instant selector still needs a reference row.
    let mut unresolved: BTreeSet<_> = inputs
        .iter()
        .enumerate()
        .flat_map(|(i, input)| {
            input
                .signals
                .iter()
                .filter(|(_, signal)| {
                    signal.key.kind == "cast"
                        && (!branches
                            || input.pull.phase_spans.iter().any(|&(id, lo, hi)| {
                                disabled.contains(&id)
                                    && lo <= signal.time_ms
                                    && signal.time_ms <= hi
                            }))
                })
                .map(move |(&index, _)| (i, index))
        })
        .collect();
    let helpers = instant_helpers(inputs);
    let mut reactive: BTreeSet<_> = inputs
        .iter()
        .enumerate()
        .flat_map(|(i, input)| {
            input
                .signals
                .iter()
                .filter(|(_, signal)| helpers.contains(&path_key(signal)))
                .map(move |(&index, _)| (i, index))
        })
        .collect();
    let mut before = None;
    for after in anchors.into_iter().filter(|_| branches) {
        // Start at this boss's boundary, e.g. X→P→C before its first common anchor C.
        if before.is_some_and(|i| sections.get(i) != sections.get(after)) {
            before = None;
        }
        let variants = paths(
            before.and_then(|i| slots.get(i)),
            slots.get(after).context("Missing endpoint")?,
            inputs,
            boss_clocks,
            &helpers,
            &mut reactive,
        )?;
        if variants.iter().flatten().any(|(i, signals)| {
            inputs.get(*i).is_some_and(|input| {
                signals.iter().any(|signal| {
                    input.pull.phase_spans.iter().any(|&(id, lo, hi)| {
                        disabled.contains(&id) && lo <= signal.time_ms && signal.time_ms <= hi
                    })
                })
            })
        }) {
            unresolved.extend(variants.iter().flatten().flat_map(|(i, signals)| {
                signals
                    .iter()
                    .filter_map(move |signal| Some((*i, *signal.event_indices.first()?)))
            }));
            before = Some(after);
            continue;
        }
        let start = match before {
            Some(i) => i + 1,
            None => sections
                .iter()
                .position(|id| Some(id) == sections.get(after))
                .context("Missing opening section")?,
        };
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
            let prefix = common_prefix(&variants);
            if variants.iter().flatten().any(|(_, casts)| {
                casts
                    .get(prefix)
                    .zip(casts.last())
                    .is_some_and(|(choice, endpoint)| endpoint.time_ms - choice.time_ms <= 250)
            }) {
                // Wait for a later merge, e.g. Mystery Magic's packet arrives before its same-tick Blizzard selector.
                continue;
            }
            candidates.insert(start, (after, variants));
        }
        before = Some(after);
    }
    let mut position = 0;
    let mut section = None;
    let mut occupied = 0;
    let mut pending_phase = false;
    let mut events = events.into_iter().map(Some).collect::<Vec<_>>();
    while position < slots.len() {
        let opening = boss_clocks && section != sections.get(position).copied();
        if let Some((after, variants)) = candidates.get(&position) {
            if let Err(error) = compiler.branch(variants, opening) {
                // Isolate compile failures too, e.g. an instant alias in phase 2 must not erase phase 1's branches.
                for (i, signals) in variants.iter().flatten() {
                    let input = inputs.get(*i).context("Missing rejected branch input")?;
                    rejected_phases.extend(
                        input
                            .pull
                            .phase_spans
                            .iter()
                            .filter(|&&(_, lo, hi)| {
                                signals
                                    .iter()
                                    .any(|signal| lo <= signal.time_ms && signal.time_ms <= hi)
                            })
                            .map(|&(id, _, _)| id),
                    );
                }
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
            if opening {
                section = sections.get(position).copied();
            }
            position = after + 1;
            continue;
        }
        let mut entry = events
            .get_mut(position)
            .and_then(Option::take)
            .context("Missing common event")?;
        let refs = samples(slots.get(position).context("Missing common slot")?, inputs);
        if opening {
            let base = compiler.allocate(occupied)?;
            for &(i, signal) in &refs {
                *compiler.clocks.get_mut(i).context("Missing boss clock")? = (signal.time_ms, base);
                *compiler.origins.get_mut(i).context("Missing boss origin")? = signal.time_ms;
            }
            section = sections.get(position).copied();
        }
        if let Entry::Event {
            sync: Some(Sync::Network(sync)),
            note,
            ..
        } = &mut entry
        {
            // Reconsider drift-disabled rows only; raw collisions and unsafe source names stay disabled.
            if (boss_clocks
                && !note
                    .as_ref()
                    .is_some_and(|n| n.contains("source name cannot")))
                || (!sync.enabled
                    && note
                        .as_ref()
                        .is_some_and(|n| n.contains("observed cast falls outside")))
            {
                sync.enabled = true;
                *note = None;
            }
        }
        let mut slot: PhaseSlot =
            serde_json::from_value(slots.get(position).context("Missing common slot")?.clone())?;
        if !refs.is_empty()
            && refs.iter().all(|(_, signal)| {
                signal.key.role.ends_with("/Boss") && helpers.contains(&path_key(signal))
            })
        {
            if let Entry::Event {
                sync: Some(Sync::Network(sync)),
                note,
                ..
            } = &mut entry
            {
                sync.enabled = false;
                *note = Some("Conditional instant follow-up: observed timing only; no independent cast start.".into());
            }
            slot.evidence = "conditionalInstantHelper".into();
        }
        let same_wave = |slot| {
            let differences: Vec<_> = samples(slot, inputs)
                .iter()
                .filter_map(|(i, other)| {
                    refs.iter()
                        .find(|(owner, _)| owner == i)
                        .map(|(_, signal)| signal.time_ms.abs_diff(other.time_ms))
                })
                .collect();
            !differences.is_empty() && differences.iter().all(|&delta| delta <= 250)
        };
        let next_wave = slots.get(position + 1).is_some_and(same_wave);
        let previous_wave = position
            .checked_sub(1)
            .and_then(|i| slots.get(i))
            .is_some_and(same_wave);
        // Every member can arrive first, e.g. Chaos and Exdeath need the same pre-entry drift window.
        let pending_clocks = next_wave.then(|| compiler.clocks.clone());
        slot.alignment_slot = Some(position);
        occupied = compiler.emit(entry, &refs, slot, None)?;
        if let Some(clocks) = pending_clocks {
            compiler.clocks = clocks;
        }
        let row = compiler.rows.last_mut().context("Missing common row")?;
        if opening {
            open_entry(row);
        }
        let wide = row
            .1
            .as_ref()
            .and_then(|s| s.window_ms)
            .is_some_and(|w| w.iter().any(|&v| v > 2500));
        // Defer isolation until after the wave, e.g. Chaos and Exdeath may arrive in either order.
        pending_phase |= wide && !opening && sync_enabled(row) && (next_wave || previous_wave);
        if (wide || pending_phase) && !opening && sync_enabled(row) && !next_wave && !previous_wave
        {
            pending_phase = false;
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
    let phase_suffix = inputs.iter().any(|input| {
        (raid || !input.pull.phase_spans.is_empty())
            && compiler
                .rows
                .iter()
                .filter_map(|(_, slot)| {
                    slot.as_ref()?
                        .samples
                        .iter()
                        .filter(|sample| sample.file == input.pull.file)
                        .map(|sample| sample.time_ms)
                        .max()
                })
                .max()
                .is_some_and(|last| {
                    input
                        .signals
                        .values()
                        .any(|signal| signal.key.kind == "cast" && signal.time_ms > last)
                })
    });
    if !boss_clocks
        && candidates.is_empty()
        && compiler.branches.is_empty()
        && compiler.phases.is_empty()
        && reactive.is_empty()
        && unresolved.is_empty()
        && !phase_suffix
    {
        return Ok((yaml, report));
    }
    reactive.extend(unresolved.iter().copied());
    let reactive_keys: BTreeSet<_> = reactive
        .iter()
        .filter_map(|&(i, index)| inputs.get(i)?.signals.get(&index).map(path_key))
        .collect();
    for (i, input) in inputs.iter().enumerate() {
        reactive.extend(
            input
                .signals
                .iter()
                .filter(|(_, signal)| {
                    signal.key.kind == "cast" && reactive_keys.contains(&path_key(signal))
                })
                .map(|(&index, _)| (i, index)),
        );
    }
    let mut optional = BTreeMap::new();
    let mut reference_rounds = BTreeMap::new();
    let mut observations = BTreeMap::new();
    for (i, input) in inputs.iter().enumerate() {
        let mut relations = BTreeMap::new();
        for (position, row) in compiler.rows.iter().enumerate() {
            let Some(slot) = &row.1 else { continue };
            if let Some(sample) = slot
                .samples
                .iter()
                .find(|sample| sample.file == input.pull.file)
            {
                observations.insert(
                    (i, position),
                    (sample.time_ms, sample.event_indices.first().copied()),
                );
                continue;
            }
            let mut matches = BTreeSet::new();
            for sample in &slot.samples {
                if !relations.contains_key(&sample.file) {
                    let source = inputs
                        .iter()
                        .find(|source| source.pull.file == sample.file)
                        .context("Missing reactive anchor source")?;
                    relations.insert(
                        sample.file.clone(),
                        super::multi::correspondence(&source.pull, &input.pull)?.0,
                    );
                }
                for target in sample
                    .event_indices
                    .iter()
                    .filter_map(|index| relations.get(&sample.file)?.matched.get(index))
                    .copied()
                {
                    if let Some(target) = Some(target)
                        .filter(|&index| {
                            input
                                .log
                                .events
                                .get(index)
                                .and_then(|event| event.ability_game_id)
                                .is_some_and(|id| slot.ability_ids.contains(&id))
                        })
                        .and_then(|index| input.signal_indices.get(&index))
                        .and_then(|index| input.signals.get(index))
                    {
                        matches.insert((target.time_ms, target.event_indices.first().copied()));
                    }
                }
            }
            if let Some(observation) = (matches.len() == 1)
                .then(|| matches.into_iter().next())
                .flatten()
            {
                // Truncated pulls still select a real path, e.g. a wipe after Future's End has no observed merge sample.
                observations.insert((i, position), observation);
            }
        }
    }
    let mut suffixes = BTreeSet::new();
    for (i, input) in inputs
        .iter()
        .enumerate()
        .filter(|(_, input)| raid || !input.pull.phase_spans.is_empty())
    {
        let covered: BTreeSet<_> = observations
            .iter()
            .filter(|((owner, _), _)| *owner == i)
            .filter_map(|(_, (_, index))| *index)
            .collect();
        let last = covered
            .iter()
            .filter_map(|index| input.signals.get(index))
            .filter(|signal| signal.key.kind == "cast")
            .map(|signal| signal.time_ms)
            .max();
        if let Some(last) = last {
            for (&index, signal) in &input.signals {
                if signal.key.kind == "cast"
                    && signal.time_ms > last
                    && !covered.contains(&index)
                    && !unresolved.contains(&(i, index))
                {
                    // EOF is not a common successor, e.g. retain Heartbreak Kick even when FFLogs supplies no phase metadata.
                    suffixes.insert((i, index));
                    reactive.insert((i, index));
                }
            }
        }
    }
    let labels: BTreeMap<_, _> = compiler
        .rows
        .iter()
        .filter_map(|(entry, _)| {
            if let Entry::Label { name, at } = entry {
                Some((name.as_str(), *at))
            } else {
                None
            }
        })
        .collect();
    for (i, index) in reactive {
        let input = inputs.get(i).context("Missing reactive input")?;
        let mut inactive = BTreeSet::new();
        for branch in &compiler.branches {
            let selected = compiler
                .rows
                .iter()
                .enumerate()
                .filter_map(|(position, (_, slot))| {
                    let slot = slot.as_ref()?;
                    if slot.selector != Some(true) {
                        return None;
                    }
                    let label = slot.path.as_ref()?;
                    if !branch.paths.iter().any(|path| &path.label == label) {
                        return None;
                    }
                    let &(at, index) = observations.get(&(i, position))?;
                    Some((at, index, label))
                })
                .min_by_key(|(at, index, _)| (*at, *index))
                .map(|(_, _, label)| label);
            inactive.extend(
                branch
                    .paths
                    .iter()
                    .filter(|path| Some(&path.label) != selected)
                    .map(|path| path.label.clone()),
            );
        }
        // Retain only missing reactions, e.g. a fan-out already covered by a common row needs no duplicate.
        if compiler.rows.iter().any(|(_, slot)| {
            slot.as_ref().is_some_and(|slot| {
                slot.samples.iter().any(|sample| {
                    sample.file == input.pull.file && sample.event_indices.contains(&index)
                })
            })
        }) {
            continue;
        }
        let signal = input
            .signals
            .get(&index)
            .context("Missing reactive signal")?;
        let (entry, origin) = compiler
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| sync_enabled(row))
            .filter_map(|(position, (entry, slot))| {
                let slot = slot.as_ref()?;
                // Counterfactual packet orders are inactive, e.g. an effect must use the actually selected X-first clock.
                if slot
                    .path
                    .as_ref()
                    .is_some_and(|path| inactive.contains(path))
                    || slot.ancestors.iter().any(|path| inactive.contains(path))
                {
                    return None;
                }
                let &(at, _) = observations.get(&(i, position))?;
                if at > signal.time_ms {
                    return None;
                }
                // Effects after dispatch use the destination clock, e.g. Launch after a branch merge.
                let clock = if let Entry::Event {
                    jump: Some(jump), ..
                } = entry
                {
                    match &jump.to {
                        Destination::Label(label) => *labels.get(label.as_str())?,
                        Destination::Time(at) => *at,
                    }
                } else {
                    time(entry)
                };
                Some((clock, at))
            })
            .max_by_key(|(_, origin)| *origin)
            // InCombat owns the initial clock, e.g. an opening reference precedes the first boss sync.
            .unwrap_or((0.0, 0));
        let key = path_key(signal);
        let round = reference_rounds
            .entry((i, (entry * 1000.0).round() as i64, key.clone()))
            .or_insert(0usize);
        let occurrence = *round;
        *round += 1;
        optional
            .entry((
                (entry * 1000.0).round() as i64,
                key,
                unresolved.contains(&(i, index)),
                suffixes.contains(&(i, index)),
                occurrence,
            ))
            .or_insert_with(Vec::new)
            .push((i, signal, origin));
    }
    for ((anchor, _, unresolved, suffix, _), refs) in optional {
        // Keep each evidence kind paired with its explanation, e.g. unresolved paths take precedence over suffixes.
        let (evidence, explanation) = if unresolved {
            (
                "unresolvedPath",
                "Unresolved path: observed timing only; path synchronization is not inferred.",
            )
        } else if suffix {
            (
                "observedSuffix",
                "Observed suffix: no common successor or continuation is inferred.",
            )
        } else {
            (
                "conditionalInstantHelper",
                "Conditional instant helper: observed timing only; no stable cast start for synchronization.",
            )
        };
        let samples = refs.iter().map(|&(i, signal, _)| (i, signal)).collect();
        let (mut entry, mut slot) = compiler.branch_entry(&samples)?;
        let timing = statistics(
            refs.iter()
                .map(|(_, signal, origin)| anchor + signal.time_ms - origin)
                .collect(),
        )?;
        let at = (timing.median_ms / 100.0).round() as i64 * 100;
        if let Entry::Event {
            at: entry_at,
            sync: Some(Sync::Network(sync)),
            note,
            ..
        } = &mut entry
        {
            *entry_at = at as f64 / 1000.0;
            sync.enabled = false;
            sync.window = Some(if unresolved || suffix {
                [
                    ((at - timing.min_ms).max(0) + 2599) as f64 / 1000.0,
                    ((timing.max_ms - at).max(0) + 2599) as f64 / 1000.0,
                ]
                .map(|ms| (ms * 10.0).ceil() / 10.0)
            } else {
                [2.5, 2.5]
            });
            slot.window_ms = sync
                .window
                .map(|window| window.map(|seconds| (seconds * 1000.0).round() as i64));
            // A reaction is visible without steering the clock, e.g. Launch has no cast start and can occur before or after missiles.
            *note = Some(explanation.into());
        }
        slot.at_ms = at.into();
        slot.evidence = evidence.into();
        for (sample, (_, signal, origin)) in slot.samples.iter_mut().zip(&refs) {
            sample.block_entry_ms = *origin;
            sample.relative_ms = signal.time_ms - origin;
        }
        let mut samples: BTreeMap<String, PhaseSample> = BTreeMap::new();
        for sample in slot.samples {
            if let Some(existing) = samples.get_mut(&sample.file) {
                existing.event_indices.extend(sample.event_indices);
                existing.instance_ids.extend(sample.instance_ids);
            } else {
                samples.insert(sample.file.clone(), sample);
            }
        }
        slot.samples = samples.into_values().collect();
        compiler.rows.push((entry, Some(slot)));
    }
    // Unstarted conditional reactions cannot forecast a mechanic, e.g. Launch depends on a player's missile hit.
    let mut excluded = Vec::new();
    compiler.rows.retain(|(_, slot)| {
        if let Some(slot) = slot
            && slot.evidence == "conditionalInstantHelper"
            && slot.selector != Some(true)
        {
            excluded.push(slot.clone());
            return false;
        }
        true
    });
    compiler
        .rows
        .sort_by(|a, b| time(&a.0).total_cmp(&time(&b.0)));
    let mut unordered = BTreeSet::new();
    for (i, (_, a)) in compiler.rows.iter().enumerate() {
        if !compiler.rows.get(i).is_some_and(sync_enabled) {
            continue;
        }
        let Some(a) = a else { continue };
        for (j, (_, b)) in
            compiler
                .rows
                .iter()
                .enumerate()
                .skip(i + 1)
                .take_while(|(_, (entry, _))| {
                    (time(entry) * 1000.0).round() as i64 - a.at_ms.as_i64().unwrap_or(0) <= 250
                })
        {
            if !compiler.rows.get(j).is_some_and(sync_enabled) {
                continue;
            }
            let Some(b) = b else { continue };
            let differences: Vec<_> = a
                .samples
                .iter()
                .filter_map(|sample| {
                    b.samples
                        .iter()
                        .find(|other| other.file == sample.file)
                        .map(|other| sample.time_ms - other.time_ms)
                })
                .collect();
            if differences.iter().all(|time| time.abs_diff(0) <= 250)
                && differences.iter().any(|&time| time > 0)
            {
                // Only observed order swaps are unordered, e.g. two Exdeath copies reverse their 45ms packet order.
                unordered.extend([i, j]);
            }
        }
    }
    for i in unordered {
        if let Some((_, Some(slot))) = compiler.rows.get_mut(i) {
            slot.evidence = "unorderedPacketWave".into();
        }
    }
    {
        let mut previous = BTreeMap::new();
        for position in 0..compiler.rows.len() {
            let row = compiler.rows.get(position).context("Missing window row")?;
            let Entry::Event {
                sync: Some(Sync::Network(sync)),
                ..
            } = &row.0
            else {
                continue;
            };
            // Disabled references cannot activate, e.g. an optional copy must not narrow a real boss sync's window.
            if !sync.enabled {
                continue;
            }
            let key = serde_json::to_string(&(&sync.log, &sync.fields))?;
            if let Some(prior) = previous.insert(key, position) {
                let (left, right) = compiler.rows.split_at_mut(position);
                let a = left.get_mut(prior).context("Missing preceding sync")?;
                let b = right.first_mut().context("Missing following sync")?;
                let gap = ((time(&b.0) - time(&a.0)) * 1000.0).round() as i64;
                if gap > 0 {
                    // Repeated packets need distinct windows, e.g. Surface Missile waves two seconds apart.
                    let limit = gap / 200 * 100;
                    for (row, edge) in [(a, 1), (b, 0)] {
                        if let (
                            Entry::Event {
                                sync: Some(Sync::Network(sync)),
                                ..
                            },
                            Some(slot),
                        ) = row
                        {
                            let mut window = sync.window.context("Missing compiled window")?;
                            if let Some(value) = window.get_mut(edge) {
                                *value = value.min(limit as f64 / 1000.0);
                            }
                            sync.window = Some(window);
                            slot.window_ms =
                                Some(window.map(|value| (value * 1000.0).round() as i64));
                        }
                    }
                }
            }
        }
    }
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
                .filter(|(_, slot)| {
                    slot.path.as_deref() == Some(path.label.as_str())
                        || slot.ancestors.contains(&path.label)
                })
                .collect();
            path.selector_slot = matching
                .iter()
                .find(|(_, s)| {
                    s.selector == Some(true) && s.path.as_deref() == Some(path.label.as_str())
                })
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
    let compiled_yaml = match draft::serialize_draft(entries, timeline.reset_on) {
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
    put(
        excluded
            .iter()
            .map(|slot| serde_json::json!({"reason":"conditionalInstantFollowup", "slot":slot}))
            .collect::<Vec<_>>(),
        &mut compiled_report,
        "excludedSignals",
    )?;
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
    // Audit independent pull clocks concurrently and retain the sorted input order in checks.
    let checks_and_samples = inputs
        .par_iter()
        .enumerate()
        .map(|(i, input)| {
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
            let mut recovered = Vec::new();
            let mut failed_phases = BTreeSet::new();
            for (position, row) in result.rows.iter().enumerate() {
                let slot = compiled_slots
                    .get(position)
                    .context("Missing replayed slot")?;
                if !result.passed
                    && matches!(
                        row.status,
                        "unverified" | "unexpectedMatch" | "missingExpectedSignal" | "windowMiss"
                    )
                {
                    for sample in &slot.samples {
                        if let Some(peer) = inputs.iter().find(|peer| peer.pull.file == sample.file)
                        {
                            failed_phases.extend(
                                peer.pull
                                    .phase_spans
                                    .iter()
                                    .filter(|&&(_, lo, hi)| {
                                        lo <= sample.time_ms && sample.time_ms <= hi
                                    })
                                    .map(|&(id, _, _)| id),
                            );
                        }
                    }
                }
                if slot
                    .samples
                    .iter()
                    .any(|sample| sample.file == input.pull.file)
                {
                    continue;
                }
                if let Some(signal) = (row.status == "matched"
                    || row.status == "observedWithoutSync")
                    .then(|| {
                        row.expected_event_indices
                            .iter()
                            .find_map(|index| input.signals.get(index))
                    })
                    .flatten()
                {
                    let origin = *slot.origins.get(i).context("Missing recovered origin")?;
                    recovered.push((
                        position,
                        PhaseSample {
                            file: input.pull.file.clone(),
                            event_indices: signal.event_indices.clone(),
                            instance_ids: signal.instance_ids.clone(),
                            ability_id: signal.key.ability_id,
                            time_ms: signal.time_ms,
                            block_entry_ms: origin,
                            relative_ms: signal.time_ms - origin,
                        },
                    ));
                }
            }
            Ok::<_, anyhow::Error>((
                ReplayCheck {
                    file: input.pull.file.clone(),
                    passed: result.passed,
                    summary: result.summary,
                    jumps: result.jumps,
                    previews: result.previews,
                },
                recovered,
                failed_phases,
            ))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let mut checks = Vec::new();
    let mut recovered = Vec::new();
    for (check, samples, phases) in checks_and_samples {
        checks.push(check);
        recovered.push(samples);
        rejected_phases.extend(phases);
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
    // Truncated pulls still provide real prefix samples, e.g. missiles before a beam interrupted by death.
    for (position, sample) in recovered.into_iter().flatten() {
        compiled_slots
            .get_mut(position)
            .context("Missing recovered slot")?
            .samples
            .push(sample);
    }
    for slot in &mut compiled_slots {
        slot.absolute_time =
            statistics(slot.samples.iter().map(|sample| sample.time_ms).collect())?;
        slot.time = statistics(
            slot.samples
                .iter()
                .map(|sample| sample.relative_ms)
                .collect(),
        )?;
    }
    put(&compiled_slots, &mut compiled_report, "slots")?;
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
        let input = inputs
            .iter()
            .find(|input| input.pull.file == coverage["file"])
            .context("Missing coverage input")?;
        let casts: BTreeSet<_> = input
            .pull
            .occurrences
            .iter()
            .filter(|row| row.kind == "cast")
            .map(|row| row.event_index)
            .collect();
        let represented: BTreeSet<_> = compiled_slots
            .iter()
            .flat_map(|slot| &slot.samples)
            .filter(|s| coverage["file"] == s.file)
            .flat_map(|s| s.event_indices.iter().copied())
            .filter(|index| casts.contains(index))
            .collect();
        let excluded: BTreeSet<_> = excluded
            .iter()
            .flat_map(|slot| &slot.samples)
            .filter(|sample| coverage["file"] == sample.file)
            .flat_map(|sample| sample.event_indices.iter().copied())
            .filter(|index| casts.contains(index) && !represented.contains(index))
            .collect();
        put(&represented, coverage, "representedEventIndices")?;
        put(&excluded, coverage, "excludedEventIndices")?;
        put(
            input
                .signals
                .values()
                .filter(|s| s.key.kind == "cast")
                .flat_map(|s| &s.event_indices)
                .filter(|&&i| !represented.contains(&i) && !excluded.contains(&i))
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
                                    || item["excludedEventIndices"]
                                        .as_array()
                                        .is_some_and(|excluded| excluded.contains(index))
                            })
                        })
                })
            })
        });
    Ok((compiled_yaml, compiled_report))
}

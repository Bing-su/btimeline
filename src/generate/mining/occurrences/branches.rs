use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::{Branch, BranchOption, Common, Event, Group, Pull, Signal, interval, trace};

type Samples = BTreeMap<String, i64>;

fn event(groups: &[&Group<'_>], pulls: &[&Pull], signal: &Signal) -> Result<Event> {
    let rows = groups
        .iter()
        .map(|group| {
            group
                .unique(signal)
                .context("Repeated identity inside branch group")
        })
        .collect::<Result<Vec<_>>>()?;
    let samples = pulls
        .iter()
        .zip(&rows)
        .map(|(pull, row)| (trace(pull), row.at))
        .collect();
    let batches: BTreeMap<_, _> = pulls
        .iter()
        .zip(&rows)
        .map(|(pull, row)| (trace(pull), row.batch))
        .collect();
    Ok(Event {
        signal: signal.clone(),
        samples,
        occurrence: Some(rows.first().context("Missing branch observation")?.ordinal),
        evidence: json!({"method":"local-order-branch-v1","batches":batches,"sync":"omitted: option-relative display event"}),
    })
}

fn events(
    paths: &[&Vec<Group<'_>>],
    pulls: &[&Pull],
    skip: usize,
    take: usize,
) -> Result<Vec<Event>> {
    let reference = paths.first().context("Missing branch path")?;
    let mut result = Vec::new();
    for (index, group) in reference.iter().enumerate().skip(skip).take(take) {
        let groups = paths
            .iter()
            .map(|path| path.get(index).context("Missing branch group"))
            .collect::<Result<Vec<_>>>()?;
        for signal in group
            .key
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|signal| groups.iter().all(|group| group.unique(signal).is_some()))
        {
            result.push(event(&groups, pulls, signal)?);
        }
    }
    Ok(result)
}

fn contains(samples: &Samples, start: Option<&Samples>, ends: &Samples) -> bool {
    samples.iter().any(|(id, at)| {
        start
            .and_then(|start| start.get(id))
            .is_none_or(|left| at > left)
            && ends.get(id).is_some_and(|end| at < end)
    })
}

fn competing_window(
    pull: &Pull,
    trigger: &Signal,
    at: i64,
    triggers: &BTreeSet<&Signal>,
) -> (Option<i64>, Option<i64>) {
    let frames = || {
        pull.batches.iter().filter(|batch| {
            batch
                .signals
                .iter()
                .any(|signal| triggers.contains(signal) && signal != trigger)
        })
    };
    let before = frames()
        .filter(|frame| frame.at_ms < at)
        .map(|frame| (frame.at_ms + 50) / 100 + 1)
        .max();
    let after = frames()
        .filter(|frame| frame.at_ms >= at)
        .map(|frame| (frame.at_ms + 50) / 100)
        .min();
    (before, after)
}

fn window(
    options: &[BranchOption],
    pulls: &[&Pull],
    ends: &Samples,
    opening: bool,
) -> Result<(i64, i64)> {
    let triggers: BTreeSet<_> = options.iter().map(|option| &option.trigger).collect();
    let times: Vec<_> = options
        .iter()
        .flat_map(|option| option.triggers.values())
        .map(|at| (at + 50) / 100)
        .collect();
    let earliest = *times.iter().min().context("Missing choice times")?;
    let latest = *times.iter().max().context("Missing choice times")?;
    let tolerance = if opening { 100 } else { 25 };
    let mut open = (earliest - tolerance).max(0);
    let mut close = latest + tolerance;
    for option in options {
        let last = option
            .events
            .iter()
            .map(|event| {
                let mut offsets = event
                    .samples
                    .iter()
                    .map(|(id, at)| {
                        Ok(at - option.triggers.get(id).context("Missing choice sample")?)
                    })
                    .collect::<Result<Vec<i64>>>()?;
                offsets.sort_unstable();
                Ok((offsets
                    .get((offsets.len() - 1) / 2)
                    .context("Missing event offsets")?
                    + 50)
                    / 100)
            })
            .collect::<Result<Vec<i64>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        for (id, at) in &option.triggers {
            close = close.min((ends.get(id).context("Missing join sample")? + 50) / 100 - last);
            let pull = pulls
                .iter()
                .find(|pull| trace(pull) == *id)
                .context("Missing choice trace")?;
            // Inspect the whole trace: a competing choice may precede this local interval.
            let (before, after) = competing_window(pull, &option.trigger, *at, &triggers);
            open = open.max(before.unwrap_or(open));
            close = close.min(after.unwrap_or(close));
        }
    }
    ensure!(
        open <= earliest && close > latest,
        "No choice window before competing signals or continuation"
    );
    Ok((open, close))
}

fn option(
    paths: &[Vec<Group<'_>>],
    pulls: &[&Pull],
    trigger: &Signal,
    indices: &[(usize, usize)],
    join: &Signal,
) -> Result<BranchOption> {
    let selected_pulls = indices
        .iter()
        .map(|(index, _)| pulls.get(*index).copied().context("Missing pull"))
        .collect::<Result<Vec<_>>>()?;
    let openings = indices
        .iter()
        .map(|(i, j)| {
            paths
                .get(*i)
                .and_then(|path| path.get(*j))
                .context("Missing opening group")
        })
        .collect::<Result<Vec<_>>>()?;
    let opening = event(&openings, &selected_pulls, trigger)?;
    let triggers = opening.samples.clone();
    let tails = indices
        .iter()
        .map(|(i, j)| {
            paths
                .get(*i)
                .and_then(|path| path.get(j + 1..))
                .context("Missing option suffix")
        })
        .collect::<Result<Vec<_>>>()?;
    let mut common = Common {
        events: vec![opening],
        branches: Vec::new(),
        diagnostics: Vec::new(),
    };
    super::expand_interval(
        &selected_pulls,
        &tails,
        json!({"branch_trigger":trigger,"join":join}),
        &mut common,
    )?;
    Ok(BranchOption {
        trigger: trigger.clone(),
        triggers,
        events: common.events,
    })
}

fn candidate(
    pulls: &[&Pull],
    original: &BTreeMap<Signal, Samples>,
    start: Option<&Samples>,
    join: &Signal,
    ends: &Samples,
    include: &impl Fn(&Signal) -> bool,
) -> Result<(Branch, Vec<Event>)> {
    let source = join.source.as_ref().context("Missing branch source")?;
    let paths = pulls
        .iter()
        .map(|pull| {
            let id = trace(pull);
            let left = start
                .and_then(|start| start.get(&id))
                .copied()
                .unwrap_or(-101);
            interval(
                pull,
                left,
                Some(*ends.get(&id).context("Missing join time")?),
                &join.source,
                include,
            )
            .map_err(anyhow::Error::msg)
        })
        .collect::<Result<Vec<_>>>()?;
    let first = paths.first().context("Missing branch reference")?;
    let prefix = first
        .iter()
        .enumerate()
        .take_while(|(index, group)| {
            paths
                .iter()
                .all(|path| path.get(*index).is_some_and(|other| other.key == group.key))
        })
        .count();
    let mut variants = BTreeMap::<&Signal, Vec<(usize, usize)>>::new();
    for (index, path) in paths.iter().enumerate() {
        let (position, group) = path
            .iter()
            .enumerate()
            .find(|(_, group)| {
                group
                    .key
                    .iter()
                    .any(|signal| original.contains_key(*signal))
            })
            .context("Alternative has no observable common choice")?;
        let choices: Vec<_> = group
            .key
            .iter()
            .copied()
            .filter(|signal| original.contains_key(*signal))
            .collect();
        let [trigger] = choices.as_slice() else {
            anyhow::bail!("Unordered common choice signals")
        };
        variants
            .entry(*trigger)
            .or_default()
            .push((index, position));
    }
    ensure!(variants.len() >= 2, "No local order alternatives");
    let mut options = Vec::new();
    for (trigger, indices) in variants {
        options.push(option(&paths, pulls, trigger, &indices, join)?);
    }
    let (open, close) = window(&options, pulls, ends, start.is_none())?;
    let prefix_paths: Vec<_> = paths.iter().collect();
    let shared = events(&prefix_paths, pulls, 0, prefix)?;
    // Only the common leading groups live outside the branch; every remainder belongs to its option.
    Ok((
        Branch {
            source: source.clone(),
            join: join.clone(),
            open,
            close,
            options,
        },
        shared,
    ))
}

// shortcut: emit one local order choice per raid; later choices need correlated selection histories.
pub(super) fn discover(
    pulls: &[&Pull],
    original: &BTreeMap<Signal, Samples>,
    stable: &BTreeMap<Signal, Samples>,
    include: &impl Fn(&Signal) -> bool,
    raid: bool,
    result: &mut Common,
) -> Result<()> {
    let moved: Vec<_> = original
        .keys()
        .filter(|signal| !stable.contains_key(*signal))
        .collect();
    if moved.is_empty() {
        return Ok(());
    }
    result.diagnostics.push(json!({"method":"consensus-backbone-v1","outside_backbone":moved,"samples":moved.iter().filter_map(|signal|original.get(*signal).map(|samples|json!({"activity":signal,"samples":samples}))).collect::<Vec<_>>()}));
    let mut sources = BTreeMap::<_, Vec<_>>::new();
    for (signal, samples) in stable {
        sources
            .entry(&signal.source)
            .or_default()
            .push((signal, samples));
    }
    let reference = trace(pulls.first().context("Missing reference")?);
    for nodes in sources.values_mut() {
        nodes.sort_by_key(|(_, samples)| samples.get(&reference));
        for (index, (join, ends)) in nodes.iter().enumerate() {
            let start = index
                .checked_sub(1)
                .and_then(|previous| nodes.get(previous))
                .map(|(_, samples)| *samples);
            let relevant = moved
                .iter()
                .filter(|signal| signal.source == join.source)
                .filter_map(|signal| original.get(*signal))
                .any(|samples| contains(samples, start, ends));
            if !relevant {
                continue;
            }
            if !raid || !result.branches.is_empty() {
                result.diagnostics.push(json!({"method":"local-order-branch-v1","join":join,"status":"unresolved","reason":"additional or non-raid choice requires phase and history support"}));
                continue;
            }
            match candidate(pulls,original,start,join,ends,include) {
                Ok((branch,shared))=>{
                    result.events.retain(|event|event.signal.source!=join.source || !contains(&event.samples,start,ends));
                    result.events.extend(shared);
                    result.diagnostics.push(json!({"method":"local-order-branch-v1","join":join,"status":"generated",
                        "start_samples":start,"join_samples":ends,"open_ms":branch.open*100,"close_ms":branch.close*100,
                        "options":branch.options.iter().map(|option|json!({"trigger":option.trigger,"traces":option.triggers.keys().collect::<Vec<_>>()})).collect::<Vec<_>>()}));
                    result.branches.push(branch);
                },
                Err(error)=>result.diagnostics.push(json!({"method":"local-order-branch-v1","join":join,"status":"unresolved","reason":error.to_string()})),
            }
        }
    }
    Ok(())
}

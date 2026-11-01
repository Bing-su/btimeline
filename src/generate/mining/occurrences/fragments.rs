use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Group, Pull, Signal, alignment, trace};

fn disjoint(group: &Group<'_>, signal: &Signal) -> bool {
    let mut entities = BTreeSet::new();
    let mut times = Vec::new();
    for row in group.observations.iter().filter(|row| row.signal == signal) {
        if row.entities.is_empty() || row.entities.iter().any(|entity| !entities.insert(*entity)) {
            return false;
        }
        times.push(row.at);
    }
    matches!((times.first(),times.last()),(Some(first),Some(last)) if last-first<=super::super::common::ORDER_RESOLUTION_MS)
}

// Only a forced peer group with one exact-time wave can support coalescing disjoint instances.
pub(super) fn normalize(rows: &mut [Vec<Group<'_>>]) -> Result<()> {
    let Some(reference) = rows.first().filter(|_| rows.len() >= 2) else {
        return Ok(());
    };
    // Ordinary intervals need only the existing final alignment, not a second candidate pass.
    if !rows.iter().flatten().any(|group| {
        group
            .key
            .windows(2)
            .any(|pair| matches!(pair,[a,b] if a==b))
    }) {
        return Ok(());
    }
    if rows
        .iter()
        .any(|row| !alignment::within_limit(reference.len(), row.len()))
    {
        return Ok(());
    }
    // Ignore multiplicity only for candidate correspondence; final alignment rechecks the keys.
    let keys: Vec<Vec<_>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|group| group.key.iter().copied().collect::<BTreeSet<_>>())
                .collect()
        })
        .collect();
    let reference = keys.first().context("Missing fragment reference")?;
    let matches = keys
        .iter()
        .map(|key| alignment::align(reference, key))
        .collect::<Result<Vec<_>>>()?;
    for (index, key) in reference.iter().enumerate() {
        let peers = matches
            .iter()
            .map(|aligned| {
                aligned
                    .pairs
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map(|(_, j)| *j)
            })
            .collect::<Option<Vec<_>>>();
        let Some(peers) = peers else { continue };
        for signal in key {
            let groups = rows
                .iter()
                .zip(&peers)
                .map(|(row, j)| row.get(*j).context("Missing fragment group"))
                .collect::<Result<Vec<_>>>()?;
            let counts: Vec<_> = groups
                .iter()
                .map(|group| group.key.iter().filter(|value| *value == signal).count())
                .collect();
            if !counts.contains(&1)
                || !counts.iter().any(|count| *count > 1)
                || !groups.iter().all(|group| disjoint(group, signal))
            {
                continue;
            }
            for (row, j) in rows.iter_mut().zip(&peers) {
                let group = row.get_mut(*j).context("Missing mutable fragment group")?;
                group.key.retain(|value| *value != *signal);
                group.key.push(signal);
                group.key.sort_unstable();
            }
        }
    }
    Ok(())
}

pub(super) fn evidence(pulls: &[&Pull], groups: &[&Group<'_>], signal: &Signal) -> Option<Value> {
    if !groups.iter().any(|group| {
        group
            .observations
            .iter()
            .filter(|row| row.signal == signal)
            .count()
            > 1
    }) {
        return None;
    }
    let rows:BTreeMap<_,_> = pulls.iter().zip(groups).map(|(pull,group)| {
        let members:Vec<_> = group.observations.iter().filter(|row|row.signal==signal).collect();
        let batches:BTreeSet<_> = members.iter().map(|row|row.batch).collect();
        let entities:BTreeSet<_> = members.iter().flat_map(|row|row.entities.iter()).collect();
        (trace(pull),json!({"batches":batches,"at_ms":members.iter().map(|row|row.at).collect::<Vec<_>>(),"instances":entities.len()}))
    }).collect();
    Some(json!(rows))
}

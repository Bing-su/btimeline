//! Keep unselected casts visible without changing the predictive score or filtering observations.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::fflogs::model::CollectedLog;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Audit {
    pub declared_sources: Vec<String>,
    pub observed_casts: usize,
    pub target_casts: usize,
    pub ignored_casts: usize,
    pub unclassified_casts: usize,
    pub unclassified: Vec<Unclassified>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Unclassified {
    pub source: String,
    pub id: String,
    pub name: Option<String>,
    pub count: usize,
    pub first_raw_index: usize,
    pub last_raw_index: usize,
    pub first_at_ms: i64,
    pub last_at_ms: i64,
}

pub(super) fn collect(
    log: &CollectedLog,
    range: std::ops::RangeInclusive<i64>,
    origin: i64,
    sources: &BTreeSet<&str>,
    selected: impl Fn(&str, &str) -> bool,
    ignored: &BTreeMap<String, String>,
) -> Result<Audit> {
    let actors = log
        .report
        .master_data
        .actors
        .iter()
        .map(|a| (a.id, a.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let names = log
        .report
        .master_data
        .abilities
        .iter()
        .map(|a| (a.game_id, a.name.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut audit = Audit {
        declared_sources: sources.iter().map(|source| (*source).to_owned()).collect(),
        ..Audit::default()
    };
    let mut unknown = BTreeMap::<(String, String), Unclassified>::new();
    for (raw_index, event) in log
        .events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == "cast" && range.contains(&e.timestamp))
    {
        let Some(source) = event
            .source_id
            .and_then(|id| actors.get(&id))
            .filter(|source| sources.contains(**source))
        else {
            continue;
        };
        let id = super::projection::ability(event)?;
        audit.observed_casts += 1;
        if selected(source, &id) {
            audit.target_casts += 1;
            continue;
        }
        if ignored.contains_key(&id) {
            audit.ignored_casts += 1;
            continue;
        }
        audit.unclassified_casts += 1;
        let row = unknown
            .entry(((*source).to_owned(), id.clone()))
            .or_insert_with(|| Unclassified {
                source: (*source).to_owned(),
                id,
                name: event
                    .ability_game_id
                    .and_then(|id| names.get(&id))
                    .map(|name| (*name).to_owned()),
                count: 0,
                first_raw_index: raw_index,
                last_raw_index: raw_index,
                first_at_ms: event.timestamp - origin,
                last_at_ms: event.timestamp - origin,
            });
        row.count += 1;
        row.last_raw_index = raw_index;
        row.last_at_ms = event.timestamp - origin;
    }
    audit.unclassified = unknown.into_values().collect();
    Ok(audit)
}

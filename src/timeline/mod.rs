mod catalog;
mod fflogs;
mod parse;
mod replay;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

pub(crate) use catalog::{CastEntry, Catalog, EventData, ExecutionContext, LoopDefinition, Owner};
pub(crate) use fflogs::from_fflogs;
pub(crate) use parse::parse;
use regress::Regex;
pub(crate) use replay::forecast::forecast;
pub(crate) use replay::{Observation, Record, Recording, replay};

// Keep authored time in integer tenths; raw observations retain milliseconds.
pub(crate) type Tick = i64;
pub(crate) const MAX_MS: i64 = 10_000_000_000_000;

#[derive(Debug)]
pub(crate) enum Field {
    Literal(Vec<String>),
    Regex(Vec<Regex>),
}

#[derive(Debug)]
pub(crate) struct Signal {
    kind: String,
    fields: BTreeMap<String, Field>,
    pub(crate) at: Option<Tick>,
    before: Tick,
    after: Option<Tick>,
    pub(crate) from: Option<String>,
    pub(crate) resume_at: Option<Tick>,
}

impl Signal {
    fn matches(&self, kind: &str, fields: &BTreeMap<String, String>, time: Tick) -> bool {
        self.kind == kind
            && self.at.is_none_or(|at| {
                time >= at - self.before && self.after.is_none_or(|after| time <= at + after)
            })
            && self.fields.iter().all(|(name, condition)| {
                fields.get(name).is_some_and(|value| match condition {
                    Field::Literal(values) => values.contains(value),
                    Field::Regex(patterns) => patterns.iter().any(|p| p.find(value).is_some()),
                })
            })
    }
}

#[derive(Debug)]
pub(crate) struct Event {
    order: usize,
    id: String,
    at: Tick,
    name: String,
    duration: Tick,
    visible: bool,
    syncs: Vec<Signal>,
}

#[derive(Debug)]
pub(crate) struct Phase {
    id: String,
    enters: Vec<Signal>,
    events: Vec<Event>,
    branches: Vec<Branch>,
    loops: Vec<Loop>,
}

#[derive(Debug)]
struct Branch {
    id: String,
    open: Tick,
    close: Tick,
    options: Vec<BranchOption>,
    merge: Option<Merge>,
}

#[derive(Debug)]
struct BranchOption {
    id: String,
    fallback: bool,
    whens: Vec<Signal>,
    requires: Vec<(String, String)>,
    events: Vec<Event>,
    loops: Vec<Loop>,
}

#[derive(Debug)]
struct Loop {
    order: usize,
    id: String,
    start: Tick,
    every: Tick,
    count: Option<i64>,
    untils: Vec<Signal>,
    events: Vec<Event>,
    branches: Vec<Branch>,
}

#[derive(Debug)]
struct Merge {
    whens: Vec<Signal>,
    events: Vec<Event>,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    pub(crate) name: String,
    lifecycle: BTreeMap<String, Vec<Signal>>,
    events: Vec<Event>,
    phases: Vec<Phase>,
    pub(crate) event_count: usize,
    pub(crate) catalog: Catalog,
}

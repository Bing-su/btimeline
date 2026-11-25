mod catalog;
mod fflogs;
mod parse;
mod replay;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

pub(crate) use catalog::{CastEntry, Catalog, EventData, ExecutionContext, LoopDefinition, Owner};
pub(crate) use fflogs::from_fflogs;
pub(crate) use parse::{expanded, parse, parse_with_file};
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

impl Field {
    fn matches(&self, value: &str) -> bool {
        match self {
            Self::Literal(values) => values.iter().any(|candidate| candidate == value),
            Self::Regex(patterns) => patterns.iter().any(|pattern| pattern.find(value).is_some()),
        }
    }
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
    fn window_end(&self) -> Option<Tick> {
        Some(self.at? + self.after?)
    }

    fn in_window(&self, time: Tick) -> bool {
        self.at.is_none_or(|at| {
            time >= at - self.before && self.window_end().is_none_or(|end| time < end)
        })
    }

    fn matches(&self, kind: &str, fields: &BTreeMap<String, String>, time: Tick) -> bool {
        self.kind == kind
            && self.in_window(time)
            && self.fields.iter().all(|(name, condition)| {
                fields
                    .get(name)
                    .is_some_and(|value| condition.matches(value))
            })
    }
}

#[derive(Debug)]
pub(crate) struct Event {
    order: usize,
    source_start: usize,
    source_end: usize,
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
    sequences: Vec<Sequence>,
    after: Vec<PhaseAfter>,
    syncs: Vec<(usize, Signal)>,
}

#[derive(Debug)]
struct PhaseAfter {
    from: String,
    at: Tick,
}

#[derive(Debug)]
struct Sequence {
    id: String,
    starts: Vec<Signal>,
    events: Vec<Event>,
    syncs: Vec<(usize, Signal)>,
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
    syncs: Vec<(usize, Signal)>,
    id: String,
    fallback: bool,
    whens: Vec<Signal>,
    requires: Vec<(String, String)>,
    events: Vec<Event>,
    loops: Vec<Loop>,
}

#[derive(Debug)]
struct Loop {
    syncs: Vec<(usize, Signal)>,
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
    syncs: Vec<(usize, Signal)>,
    whens: Vec<Signal>,
    events: Vec<Event>,
}

#[derive(Debug)]
pub(crate) struct Choice {
    id: String,
    step_id: String,
    at: Tick,
    option_ids: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    choices: Vec<Choice>,
    pub(crate) name: String,
    lifecycle: BTreeMap<String, Vec<Signal>>,
    events: Vec<Event>,
    syncs: Vec<(usize, Signal)>,
    phases: Vec<Phase>,
    pub(crate) event_count: usize,
    pub(crate) catalog: Catalog,
}

mod parse;
mod replay;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

pub(crate) use parse::parse;
use regress::Regex;
pub(crate) use replay::{Recording, replay};

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
}

#[derive(Debug)]
pub(crate) struct Timeline {
    pub(crate) name: String,
    lifecycle: BTreeMap<String, Vec<Signal>>,
    events: Vec<Event>,
    phases: Vec<Phase>,
    // Preserve unsupported controls so replay cannot silently validate a reduced timeline.
    controls: Vec<String>,
    pub(crate) event_count: usize,
}

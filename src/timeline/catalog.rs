//! Stable definition ownership for replay identities and authoring profiles; no execution rules live here.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, Serializer};

use super::{Branch, Event, Field, Loop, Signal, Timeline};

#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Deserialize, Serialize)]
pub(crate) struct Owner {
    pub phase: Option<String>,
    pub loop_id: Option<String>,
    pub branch: Option<String>,
    pub option: Option<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub(crate) struct ExecutionContext {
    #[serde(flatten)]
    pub owner: Owner,
    pub phase_instance: usize,
    pub iteration: Option<u64>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EventData {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub duration_ms: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct EventDefinition {
    #[serde(flatten)]
    pub event: EventData,
    pub at_ms: i64,
    pub owner: Owner,
    #[serde(skip)]
    pub cast: Option<(String, String)>,
}
#[derive(Debug, Serialize)]
pub(crate) struct BranchDefinition {
    pub id: String,
    pub owner: Owner,
    pub options: Vec<String>,
    pub fallback_options: Vec<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct LoopDefinition {
    pub id: String,
    pub owner: Owner,
    pub anchor_events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}
#[derive(Debug, Default, Serialize)]
pub(crate) struct Catalog {
    #[serde(serialize_with = "event_values")]
    pub events: BTreeMap<usize, EventDefinition>,
    pub branches: Vec<BranchDefinition>,
    pub loops: Vec<LoopDefinition>,
}
fn event_values<S: Serializer>(
    events: &BTreeMap<usize, EventDefinition>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    events.values().collect::<Vec<_>>().serialize(serializer)
}
impl Catalog {
    pub(super) fn from_timeline(timeline: &Timeline) -> Self {
        let mut catalog = Self::default();
        catalog.body(&timeline.events, &[], &[], &Owner::default());
        for phase in &timeline.phases {
            catalog.body(
                &phase.events,
                &phase.branches,
                &phase.loops,
                &Owner {
                    phase: Some(phase.id.clone()),
                    ..Owner::default()
                },
            );
        }
        catalog
    }

    fn body(&mut self, events: &[Event], branches: &[Branch], loops: &[Loop], owner: &Owner) {
        for event in events {
            self.events.insert(
                event.order,
                EventDefinition {
                    event: EventData {
                        id: event.id.clone(),
                        name: event.name.clone(),
                        visible: event.visible,
                        duration_ms: event.duration * 100,
                    },
                    at_ms: event.at * 100,
                    owner: owner.clone(),
                    cast: impact_cast(event),
                },
            );
        }
        for branch in branches {
            self.branches.push(BranchDefinition {
                id: branch.id.clone(),
                owner: owner.clone(),
                options: branch.options.iter().map(|o| o.id.clone()).collect(),
                fallback_options: branch
                    .options
                    .iter()
                    .filter(|o| o.fallback)
                    .map(|o| o.id.clone())
                    .collect(),
            });
            for option in &branch.options {
                let next = Owner {
                    branch: Some(branch.id.clone()),
                    option: Some(option.id.clone()),
                    ..owner.clone()
                };
                self.body(&option.events, &[], &option.loops, &next);
            }
            if let Some(merge) = &branch.merge {
                self.body(
                    &merge.events,
                    &[],
                    &[],
                    &Owner {
                        branch: Some(branch.id.clone()),
                        option: None,
                        ..owner.clone()
                    },
                );
            }
        }
        for repeat in loops {
            self.loops.push(LoopDefinition {
                id: repeat.id.clone(),
                count: repeat.count,
                owner: owner.clone(),
                anchor_events: repeat
                    .events
                    .iter()
                    .filter(|e| e.visible && e.at == 0)
                    .map(|e| e.id.clone())
                    .collect(),
            });
            let next = Owner {
                loop_id: Some(repeat.id.clone()),
                ..owner.clone()
            };
            self.body(&repeat.events, &repeat.branches, &[], &next);
        }
    }
}

fn impact_cast(event: &Event) -> Option<(String, String)> {
    let mut impacts = event
        .syncs
        .iter()
        .filter(|signal| signal.kind == "Ability" && signal.at == Some(event.at));
    let binding = literal_cast(impacts.next()?)?;
    impacts
        .all(|signal| literal_cast(signal) == Some(binding))
        .then(|| (binding.0.into(), binding.1.into()))
}

fn literal_cast(signal: &Signal) -> Option<(&str, &str)> {
    if signal.kind != "Ability" {
        return None;
    }
    literal_cast_fields(signal)
}

fn literal_cast_fields(signal: &Signal) -> Option<(&str, &str)> {
    if signal.fields.len() != 2 {
        return None;
    }
    let field = |name| match signal.fields.get(name)? {
        Field::Literal(values) => match values.as_slice() {
            [value] => Some(value.as_str()),
            _ => None,
        },
        Field::Regex(_) => None,
    };
    let id = field("id")?;
    let number = i64::from_str_radix(id, 16).ok()?;
    (number >= 0 && format!("{number:X}") == id).then_some((id, field("source")?))
}

impl Timeline {
    pub(crate) fn cast_entries(&self) -> impl Iterator<Item = CastEntry<'_>> {
        self.phases.iter().filter_map(cast_entry)
    }
}

pub(crate) struct CastEntry<'a> {
    pub phase: &'a str,
    pub source: &'a str,
    pub raw_kind: &'static str,
    pub entries: Vec<(Option<&'a str>, String)>,
}

fn cast_entry(phase: &super::Phase) -> Option<CastEntry<'_>> {
    let first = phase.enters.first()?;
    let (_, source) = literal_cast_fields(first)?;
    let raw_kind = match first.kind.as_str() {
        "Ability" => "cast",
        "StartsUsing" => "begincast",
        _ => return None,
    };
    let mut entries = Vec::new();
    for signal in &phase.enters {
        let (id, other_source) = literal_cast_fields(signal)?;
        if other_source != source
            || signal.kind != first.kind
            // A mixed root and transition would need separate inferred boundaries.
            || signal.from.is_some() != first.from.is_some()
            || signal.resume_at.is_some_and(|offset| offset != 0)
        {
            return None;
        }
        entries.push((signal.from.as_deref(), id.into()));
    }
    Some(CastEntry {
        phase: &phase.id,
        source,
        raw_kind,
        entries,
    })
}

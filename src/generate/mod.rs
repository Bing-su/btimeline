mod input;
mod mining;
mod pairing;
mod render;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use path_slash::PathExt as _;
use serde::{Deserialize, Serialize};

// Align equivalent FFLogs segments; an encounter ID can identify one checkpoint segment of a battle.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupKey {
    pub(crate) encounter: i64,
    pub(crate) difficulty: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Occurrence {
    pub event_index: usize,
    pub timestamp_ms: i64,
    pub relative_ms: i64,
    pub simultaneous: usize,
    pub kind: String,
    pub actor_id: i64,
    pub actor_game_id: i64,
    pub role: String,
    pub instance: Option<i64>,
    pub ability_id: i64,
    pub start_event_index: Option<usize>,
    pub start_timestamp_ms: Option<i64>,
    pub completion_event_index: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PhaseTransition {
    pub id: i64,
    pub timestamp_ms: i64,
    pub relative_ms: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Pull {
    pub file: String,
    pub report: String,
    pub revision: i64,
    pub game_version: i64,
    pub log_version: i64,
    pub fight: i64,
    pub name: String,
    pub kill: bool,
    pub end_ms: i64,
    pub event_count: usize,
    pub phase_transitions: Vec<PhaseTransition>,
    // Share immutable evidence across replay variants, e.g. packet auditing restores the raw slice.
    pub occurrences: Arc<[Occurrence]>,
    #[serde(skip)]
    actors: BTreeMap<i64, (i64, String)>,
    #[serde(skip)]
    virtual_actors: BTreeMap<i64, String>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub key: GroupKey,
    pub pulls: Vec<Pull>,
}

// Keep normalized and raw events from one read, e.g. pairing and sync checks share source indices.
struct Source {
    key: GroupKey,
    pull: Pull,
}

fn slash_path(path: &Path) -> String {
    let path = path.to_slash_lossy();
    // path-slash retains Windows prefixes; normalize e.g. \\server\share too, preserving Unix filenames.
    #[cfg(windows)]
    {
        path.replace('\\', "/")
    }
    #[cfg(not(windows))]
    {
        path.into_owned()
    }
}

pub use input::inspect;
pub(crate) use input::validated_collected;
pub(crate) use mining::mine;
pub(crate) use render::generate;

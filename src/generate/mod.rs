mod input;
mod pairing;
mod sections;

use std::collections::BTreeMap;
use std::path::Path;

use input::load_one;
use path_slash::PathExt as _;
use serde::{Deserialize, Serialize};

use crate::fflogs::model::CollectedLog;

// Align equivalent FFLogs segments; an encounter ID can identify one checkpoint segment of a battle.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct GroupKey {
    encounter: i64,
    difficulty: i64,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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
    pub occurrences: Vec<Occurrence>,
    #[serde(skip)]
    actors: BTreeMap<i64, (i64, String)>,
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
    log: CollectedLog,
    sha256: String,
}

// Bind evidence to the bytes actually parsed, e.g. an added friendly cast invalidates the old hash too.
fn sha256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

mod alignment;
mod draft;
mod multi;
mod phase;
mod repeat;
pub(crate) mod replay;
mod report;
pub use alignment::align;
#[cfg(test)]
use draft::generate;
pub use draft::{GenerateMode, generate_selected};
pub use input::inspect;
pub use report::markdown_file;

#[cfg(test)]
mod tests;

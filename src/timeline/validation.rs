use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};

use super::{Destination, Entry, FieldPattern, JumpWhen, LogType, Sync, Timeline};

fn check_pattern(pattern: &str) -> Result<()> {
    regress::Regex::with_flags(pattern, "i")
        .with_context(|| format!("Invalid regex: {pattern}"))?;
    Ok(())
}

impl Timeline {
    pub(super) fn validate_relations(&self) -> Result<()> {
        let mut labels = BTreeSet::new();
        for entry in &self.entries {
            if let Entry::Label { name, .. } = entry {
                ensure!(labels.insert(name), "Duplicate label: {name}");
            }
        }
        let mut ordered = true;
        let mut previous = 0.0;
        for entry in &self.entries {
            match entry {
                Entry::Event { at, sync, jump, .. } => {
                    if let Some(sync) = sync {
                        sync.validate()?;
                    }
                    if let Some(jump) = jump {
                        ensure!(
                            sync.as_ref().is_some_and(Sync::enabled),
                            "Jump requires an enabled sync"
                        );
                        match &jump.to {
                            Destination::Label(to) => {
                                ensure!(labels.contains(to), "Unknown jump label: {to}")
                            }
                            Destination::Time(to) => {
                                ensure!(
                                    !matches!(jump.when, JumpWhen::Always) || *to != 0.0,
                                    "forcejump to 0 is invalid"
                                );
                            }
                        }
                    }
                    check_order(*at, ordered, &mut previous)?;
                }
                Entry::Label { at, .. } => check_order(*at, ordered, &mut previous)?,
                Entry::GeneratorOptions { .. } => {}
                Entry::SyncOrder { enabled } => {
                    if *enabled {
                        ensure!(!ordered, "Unmatched syncOrder enable");
                    } else {
                        ensure!(ordered, "Nested syncOrder disable");
                    }
                    ordered = *enabled;
                }
                Entry::AbilityCatalog { abilities, .. } => {
                    let mut ids = BTreeSet::new();
                    for ability in abilities {
                        ensure!(ids.insert(&ability.id), "Duplicate ability ID");
                    }
                }
                Entry::Note { .. } => {}
            }
        }
        ensure!(ordered, "Unclosed syncOrder disable");
        Ok(())
    }
}

fn check_order(at: f64, ordered: bool, previous: &mut f64) -> Result<()> {
    ensure!(
        !ordered || at >= *previous,
        "Timed entries are out of order"
    );
    *previous = at;
    Ok(())
}
impl Sync {
    pub(super) fn enabled(&self) -> bool {
        match self {
            Self::Network(s) => s.enabled,
            Self::Regex(s) => s.enabled,
        }
    }

    pub(super) fn window(&self) -> Option<[f64; 2]> {
        match self {
            Self::Network(s) => s.window,
            Self::Regex(s) => s.window,
        }
    }

    fn validate(&self) -> Result<()> {
        match self {
            Self::Network(sync) => {
                for value in sync.fields.values() {
                    match value {
                        FieldPattern::One(value) => check_pattern(value)?,
                        FieldPattern::Many(values) => {
                            for value in values {
                                check_pattern(value)?;
                            }
                        }
                    }
                }
            }
            Self::Regex(sync) => check_pattern(&sync.regex)?,
        }
        Ok(())
    }
}

// These field lists cover the log definitions currently needed by v1 fixtures and generation.
// Example: Ability accepts id/source; InCombat accepts inGameCombat.
pub(super) fn known_fields(log: &LogType) -> &'static [&'static str] {
    match log {
        LogType::Ability | LogType::StartsUsing => &[
            "type",
            "sourceId",
            "source",
            "id",
            "name",
            "targetId",
            "target",
            "castTime",
            "abilityGameLog",
            "flags",
            "damage",
            "targetIndex",
            "targetCount",
        ],
        LogType::InCombat => &["type", "inGameCombat", "inACTCombat"],
        LogType::GainsEffect | LogType::LosesEffect => &[
            "type", "effectId", "effect", "duration", "sourceId", "source", "targetId", "target",
            "count",
        ],
        LogType::AddedCombatant | LogType::RemovedCombatant => &[
            "type",
            "id",
            "name",
            "npcId",
            "npcNameId",
            "job",
            "level",
            "ownerId",
        ],
        LogType::Tether => &["type", "sourceId", "source", "targetId", "target", "id"],
        LogType::HeadMarker => &["type", "targetId", "target", "id"],
        LogType::GameLog => &["type", "code", "name", "message"],
        LogType::Map => &["type", "id", "regionName", "placeName"],
    }
}

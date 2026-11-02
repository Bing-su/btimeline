use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::PlannedFight;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(super) struct GameZone {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FightMetadata {
    pub game_zone: Option<GameZone>,
    pub combat_time: Option<i64>,
    pub fight_percentage: Option<f64>,
    pub boss_percentage: Option<f64>,
    pub last_phase: Option<i64>,
    pub last_phase_as_absolute_index: Option<i64>,
    pub last_phase_is_intermission: Option<bool>,
    pub has_echo: Option<bool>,
}

impl FightMetadata {
    pub fn zone_id(&self) -> Option<i64> {
        self.game_zone.as_ref().map(|zone| zone.id)
    }

    pub fn validate(&self, duration: i64) -> Result<()> {
        ensure!(
            self.game_zone
                .as_ref()
                .is_none_or(|zone| (1..=i64::from(i32::MAX)).contains(&zone.id)
                    && !zone.name.trim().is_empty()),
            "Invalid planned game zone"
        );
        ensure!(
            self.combat_time
                .is_none_or(|time| (0..=duration).contains(&time)),
            "Invalid planned combat time"
        );
        ensure!(
            [self.fight_percentage, self.boss_percentage]
                .into_iter()
                .flatten()
                .all(|value| value.is_finite() && (0.0..=100.0).contains(&value)),
            "Invalid planned fight progress"
        );
        ensure!(
            [self.last_phase, self.last_phase_as_absolute_index]
                .into_iter()
                .flatten()
                .all(|phase| phase >= 0),
            "Invalid planned phase"
        );
        Ok(())
    }
}

pub(super) const POLICY: &str = "report-cohort-outcome-long-short-v1";

// Favor independent reports and both outcomes; later rounds retain short-clear/wipe controls too.
pub(super) fn prioritize(mut candidates: Vec<PlannedFight>, limit: usize) -> Vec<PlannedFight> {
    let mut reports = BTreeMap::<String, usize>::new();
    let mut cohorts = BTreeMap::new();
    let mut outcomes = BTreeMap::new();
    let mut selected = Vec::new();
    while !candidates.is_empty() && selected.len() < limit {
        let index = candidates
            .iter()
            .enumerate()
            .min_by_key(|(_, fight)| {
                let used = reports.get(&fight.report).copied().unwrap_or(0);
                let cohort = (fight.encounter, fight.difficulty, fight.metadata.zone_id());
                let duration = fight
                    .metadata
                    .combat_time
                    .filter(|time| *time > 0)
                    .unwrap_or(fight.end_time - fight.start_time);
                (
                    used,
                    cohorts.get(&cohort).copied().unwrap_or(0),
                    outcomes.get(&(cohort, fight.kill)).copied().unwrap_or(0),
                    !fight.kill,
                    if used % 4 < 2 { -duration } else { duration },
                    &fight.report,
                    fight.fight,
                )
            })
            .map(|(index, _)| index);
        let Some(index) = index else { break };
        let fight = candidates.remove(index);
        let cohort = (fight.encounter, fight.difficulty, fight.metadata.zone_id());
        *reports.entry(fight.report.clone()).or_default() += 1;
        *cohorts.entry(cohort).or_insert(0) += 1;
        *outcomes.entry((cohort, fight.kill)).or_insert(0) += 1;
        selected.push(fight);
    }
    selected
}

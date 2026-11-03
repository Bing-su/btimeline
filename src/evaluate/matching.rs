//! Ordered one-to-one assignment inside declared candidate ID sets; concurrent sets remain independent.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::projection::Target;
use crate::timeline::Record;

#[derive(Clone, Copy, Default)]
struct Cost {
    within: usize,
    pairs: usize,
    error: u64,
}
impl Cost {
    fn rank(self) -> (usize, usize, std::cmp::Reverse<u64>) {
        (self.within, self.pairs, std::cmp::Reverse(self.error))
    }

    fn paired(self, error: u64, tolerance: i64) -> Self {
        Self {
            within: self.within + usize::from(error <= tolerance.unsigned_abs()),
            pairs: self.pairs + 1,
            error: self.error.saturating_add(error),
        }
    }
}

fn group(targets: &[&Target], events: &[&Record], tolerance: i64) -> Result<Vec<Option<usize>>> {
    let width = events.len() + 1;
    let cells = (targets.len() + 1)
        .checked_mul(width)
        .filter(|cells| *cells <= 2_000_000);
    ensure!(
        cells.is_some(),
        "Matching group exceeds two million cells; split the declared candidate set"
    );
    let mut choices = vec![0_u8; cells.expect("checked matching size")];
    let mut previous = vec![Cost::default(); width];
    for (i, target) in targets.iter().enumerate() {
        let mut current = vec![Cost::default(); width];
        for (j, event) in events.iter().enumerate() {
            let mut best = previous.get(j + 1).copied().expect("matching column");
            let mut choice = 1;
            let skipped = current.get(j).copied().expect("previous column");
            if skipped.rank() > best.rank() {
                best = skipped;
                choice = 2;
            }
            let paired = previous
                .get(j)
                .copied()
                .expect("previous column")
                .paired(event.at_ms.abs_diff(target.at_ms), tolerance);
            if paired.rank() >= best.rank() {
                best = paired;
                choice = 3;
            }
            *current.get_mut(j + 1).expect("matching column") = best;
            *choices
                .get_mut((i + 1) * width + j + 1)
                .expect("matching cell") = choice;
        }
        previous = current;
    }
    let mut pairs = vec![None; targets.len()];
    let (mut i, mut j) = (targets.len(), events.len());
    while i > 0 && j > 0 {
        match choices.get(i * width + j).copied().expect("matching cell") {
            3 => {
                *pairs.get_mut(i - 1).expect("matching target") = Some(j - 1);
                i -= 1;
                j -= 1;
            }
            2 => j -= 1,
            _ => i -= 1,
        }
    }
    Ok(pairs)
}

pub(super) fn assign(
    targets: &[Target],
    events: &[&Record],
    tolerance: i64,
) -> Result<Vec<Option<usize>>> {
    let mut groups = BTreeMap::<&[String], Vec<usize>>::new();
    for (index, target) in targets.iter().enumerate() {
        groups.entry(&target.event_ids).or_default().push(index);
    }
    let mut result = vec![None; targets.len()];
    for (ids, mut indices) in groups {
        indices.sort_by_key(|i| {
            targets
                .get(*i)
                .map(|target| (target.at_ms, target.raw_index))
        });
        let candidates = events
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.event
                    .as_ref()
                    .is_some_and(|event| ids.binary_search(&event.id).is_ok())
            })
            .collect::<Vec<_>>();
        let group_targets = indices
            .iter()
            .map(|i| targets.get(*i).expect("group target"))
            .collect::<Vec<_>>();
        let group_events = candidates
            .iter()
            .map(|(_, event)| **event)
            .collect::<Vec<_>>();
        for (target, pair) in
            indices
                .into_iter()
                .zip(group(&group_targets, &group_events, tolerance)?)
        {
            *result.get_mut(target).expect("group target") =
                pair.map(|index| candidates.get(index).expect("group event").0);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Observation, Recording};
    #[test]
    fn maximize_valid_pairs_before_assigning_a_nearby_event_to_an_earlier_bad_target() {
        let model = crate::timeline::parse(
            r#"timeline 1 "Test" {
            start "Begin"
            phase "boss" { enter "Begin"; event "one" 0.2 name="Hit"; event "two" 0.3 name="Hit"; }
        }"#,
        )
        .expect("timeline");
        let recording = Recording {
            version: 1,
            end_ms: 1000,
            observations: vec![Observation {
                at_ms: 0,
                kind: "Begin".into(),
                fields: BTreeMap::new(),
                source_event_index: None,
            }],
        };
        let rows = crate::timeline::replay(&model, &recording).expect("replay");
        let events = rows
            .iter()
            .filter(|row| row.action == "event")
            .collect::<Vec<_>>();
        let target = |at_ms| Target {
            raw_index: 0,
            at_ms,
            id: "A001".into(),
            name: "Hit".into(),
            lead_ms: None,
            event_ids: vec!["one".into(), "two".into()],
            equivalents: vec![],
        };
        let matches = assign(&[target(110), target(210)], &events, 15).expect("assignment");
        assert_eq!(matches, vec![None, Some(0)]);
    }
}

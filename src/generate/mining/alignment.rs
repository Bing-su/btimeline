use anyhow::{Context, Result, ensure};

// Bound both DP tables before allocation, e.g. long unresolved intervals remain diagnostics.
pub(super) const MAX_CELLS: usize = 1_000_000;

pub(super) fn within_limit(left: usize, right: usize) -> bool {
    left.checked_add(1)
        .and_then(|rows| right.checked_add(1).and_then(|cols| rows.checked_mul(cols)))
        .is_some_and(|cells| cells <= MAX_CELLS)
}

struct Scores {
    width: usize,
    values: Vec<u32>,
}

impl Scores {
    fn new(rows: usize, width: usize) -> Self {
        Self {
            width,
            values: vec![0; rows * width],
        }
    }

    fn get(&self, row: usize, column: usize) -> Result<u32> {
        ensure!(column < self.width, "Invalid alignment column");
        self.values
            .get(row * self.width + column)
            .copied()
            .context("Invalid alignment row")
    }

    fn set(&mut self, row: usize, column: usize, value: u32) -> Result<()> {
        ensure!(column < self.width, "Invalid alignment column");
        *self
            .values
            .get_mut(row * self.width + column)
            .context("Invalid alignment row")? = value;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub(super) struct Alignment {
    pub pairs: Vec<(usize, usize)>,
    pub ambiguous_left: Vec<usize>,
    pub reference_only: Vec<usize>,
    pub ambiguous_right: Vec<usize>,
    pub peer_only: Vec<usize>,
    pub optimal_matches: u32,
}

fn tables<T: Eq>(left: &[T], right: &[T]) -> Result<(Scores, Scores)> {
    ensure!(
        within_limit(left.len(), right.len()),
        "Alignment exceeds {MAX_CELLS} cells"
    );
    let mut prefix = Scores::new(left.len() + 1, right.len() + 1);
    for (i, a) in left.iter().enumerate() {
        for (j, b) in right.iter().enumerate() {
            let score = if a == b {
                prefix.get(i, j)? + 1
            } else {
                prefix.get(i, j + 1)?.max(prefix.get(i + 1, j)?)
            };
            prefix.set(i + 1, j + 1, score)?;
        }
    }
    let mut suffix = Scores::new(left.len() + 1, right.len() + 1);
    for (i, a) in left.iter().enumerate().rev() {
        for (j, b) in right.iter().enumerate().rev() {
            let score = if a == b {
                suffix.get(i + 1, j + 1)? + 1
            } else {
                suffix.get(i, j + 1)?.max(suffix.get(i + 1, j)?)
            };
            suffix.set(i, j, score)?;
        }
    }
    Ok((prefix, suffix))
}

// Keep only pairs present in every optimal insertion/deletion alignment, never one arbitrary backtrack.
pub(super) fn align<T: Eq>(left: &[T], right: &[T]) -> Result<Alignment> {
    let (prefix, suffix) = tables(left, right)?;
    let optimum = prefix.get(left.len(), right.len())?;
    let mut result = Alignment {
        optimal_matches: optimum,
        ..Alignment::default()
    };
    let mut possible_right = vec![false; right.len()];
    let mut forced_right = vec![false; right.len()];
    for (i, a) in left.iter().enumerate() {
        let mut candidates = Vec::new();
        for (j, b) in right.iter().enumerate() {
            if a == b && prefix.get(i, j)? + 1 + suffix.get(i + 1, j + 1)? == optimum {
                candidates.push(j);
                *possible_right
                    .get_mut(j)
                    .context("Missing candidate slot")? = true;
            }
        }
        let mut can_skip = false;
        for j in 0..=right.len() {
            can_skip |= prefix.get(i, j)? + suffix.get(i + 1, j)? == optimum;
        }
        match candidates.as_slice() {
            [] => result.reference_only.push(i),
            [j] if !can_skip => {
                result.pairs.push((i, *j));
                *forced_right.get_mut(*j).context("Missing forced slot")? = true;
            }
            _ => result.ambiguous_left.push(i),
        }
    }
    for (j, (possible, forced)) in possible_right.iter().zip(forced_right).enumerate() {
        if !possible {
            result.peer_only.push(j);
        } else if !forced {
            result.ambiguous_right.push(j);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    use super::*;

    #[rstest::rstest]
    #[case(b"ABBC",b"ABC",vec![(0,0),(3,2)],vec![1,2])]
    #[case(b"ABXBC",b"AXBC",vec![(0,0),(2,1),(3,2),(4,3)],vec![])]
    #[case(b"ABBC",b"ABBC",vec![(0,0),(1,1),(2,2),(3,3)],vec![])]
    #[case(b"AXC",b"AYC",vec![(0,0),(2,2)],vec![])]
    fn preserves_only_forced_correspondence(
        #[case] left: &[u8],
        #[case] right: &[u8],
        #[case] pairs: Vec<(usize, usize)>,
        #[case] ambiguous: Vec<usize>,
    ) {
        let result = align(left, right).unwrap();
        assert_eq!(result.pairs, pairs);
        assert_eq!(result.ambiguous_left, ambiguous);
    }

    fn matchings(
        left: &[u8],
        right: &[u8],
        i: usize,
        j: usize,
        path: Vec<(usize, usize)>,
    ) -> Vec<Vec<(usize, usize)>> {
        let mut result = vec![path.clone()];
        for (a, value) in left.iter().enumerate().skip(i) {
            for (b, _) in right
                .iter()
                .enumerate()
                .skip(j)
                .filter(|(_, other)| *other == value)
            {
                let mut next = path.clone();
                next.push((a, b));
                result.extend(matchings(left, right, a + 1, b + 1, next));
            }
        }
        result
    }

    proptest! {
        #[test]
        fn agrees_with_exhaustive_optimal_matchings(left in prop::collection::vec(0_u8..3,0..6),right in prop::collection::vec(0_u8..3,0..6)) {
            let all=matchings(&left,&right,0,0,Vec::new());
            let best=all.iter().map(Vec::len).max().unwrap();
            let mut optimal=all.iter().filter(|path|path.len()==best);
            let mut forced:BTreeSet<_>=optimal.next().unwrap().iter().copied().collect();
            for path in optimal {
                let pairs:BTreeSet<_>=path.iter().copied().collect();
                forced=forced.intersection(&pairs).copied().collect();
            }
            let actual=align(&left,&right).unwrap();
            prop_assert_eq!(actual.optimal_matches as usize,best);
            prop_assert_eq!(actual.pairs.into_iter().collect::<BTreeSet<_>>(),forced);
        }
    }

    #[test]
    fn excessive_intervals_are_rejected_before_allocation() {
        assert!(!within_limit(usize::MAX, 1));
        let left = vec![0_u8; 1000];
        align(&left, &left).unwrap_err();
    }
}

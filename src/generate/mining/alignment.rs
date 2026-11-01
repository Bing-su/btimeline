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

#[derive(Debug, Default)]
pub(super) struct AttemptAlignment {
    pub pairs: Vec<(usize, usize)>,
    pub ambiguous_left: Vec<usize>,
    pub missing_left: Vec<usize>,
    pub censored_left: Vec<usize>,
    pub peer_only: Vec<usize>,
    pub ambiguous_right: Vec<usize>,
    pub cost: u32,
    pub ends: Vec<usize>,
}

fn prefix_costs<T: Eq>(left: &[T], right: &[T]) -> Result<(Scores, Scores)> {
    ensure!(
        within_limit(left.len(), right.len()),
        "Prefix alignment exceeds {MAX_CELLS} cells"
    );
    let mut before = Scores::new(left.len() + 1, right.len() + 1);
    let mut after = Scores::new(left.len() + 1, right.len() + 1);
    for i in 0..=left.len() {
        before.set(i, 0, u32::try_from(i)?)?;
    }
    for j in 0..=right.len() {
        before.set(0, j, u32::try_from(j)?)?;
        after.set(left.len(), j, u32::try_from(right.len() - j)?)?;
    }
    for (i, a) in left.iter().enumerate() {
        for (j, b) in right.iter().enumerate() {
            let skip = before.get(i, j + 1)?.min(before.get(i + 1, j)?) + 1;
            let score = if a == b {
                skip.min(before.get(i, j)?)
            } else {
                skip
            };
            before.set(i + 1, j + 1, score)?;
        }
    }
    // Free reference suffix only after the last observed token; internal missing B still costs one.
    for (i, a) in left.iter().enumerate().rev() {
        for (j, b) in right.iter().enumerate().rev() {
            let skip = after.get(i + 1, j)?.min(after.get(i, j + 1)?) + 1;
            let score = if a == b {
                skip.min(after.get(i + 1, j + 1)?)
            } else {
                skip
            };
            after.set(i, j, score)?;
        }
    }
    Ok((before, after))
}

// Compare a terminated observation trace with any reference prefix, retaining every optimal endpoint.
pub(super) fn align_prefix<T: Eq>(left: &[T], right: &[T]) -> Result<AttemptAlignment> {
    let (before, after) = prefix_costs(left, right)?;
    let cost = after.get(0, 0)?;
    let mut result = AttemptAlignment {
        cost,
        ..AttemptAlignment::default()
    };
    for i in 0..=left.len() {
        if before.get(i, right.len())? == cost {
            result.ends.push(i);
        }
    }
    let first = *result.ends.first().context("Missing prefix endpoint")?;
    let last = *result.ends.last().context("Missing prefix endpoint")?;
    let mut possible_right = vec![false; right.len()];
    let mut forced_right = vec![false; right.len()];
    for (i, a) in left.iter().enumerate() {
        let mut candidates = Vec::new();
        for (j, b) in right.iter().enumerate() {
            if a == b && before.get(i, j)? + after.get(i + 1, j + 1)? == cost {
                candidates.push(j);
                *possible_right
                    .get_mut(j)
                    .context("Missing prefix candidate")? = true;
            }
        }
        let mut can_skip = first <= i;
        for j in 0..right.len() {
            can_skip |= before.get(i, j)? + 1 + after.get(i + 1, j)? == cost;
        }
        match candidates.as_slice() {
            [] if i >= last => result.censored_left.push(i),
            [] if i < first => result.missing_left.push(i),
            [j] if !can_skip => {
                result.pairs.push((i, *j));
                *forced_right
                    .get_mut(*j)
                    .context("Missing forced prefix slot")? = true;
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
        align_prefix(&left, &left).unwrap_err();
    }

    #[rstest::rstest]
    #[case(b"ABBC",b"AB",vec![(0,0),(1,1)],vec![],vec![],vec![2,3])]
    #[case(b"AXCDE",b"ACD",vec![(0,0),(2,1),(3,2)],vec![1],vec![],vec![4])]
    #[case(b"ABBC",b"ABC",vec![(0,0)],vec![],vec![1,2,3],vec![])]
    #[case(b"ABC",b"",vec![],vec![],vec![],vec![0,1,2])]
    fn prefix_separates_internal_gaps_from_censored_suffix(
        #[case] left: &[u8],
        #[case] right: &[u8],
        #[case] pairs: Vec<(usize, usize)>,
        #[case] missing: Vec<usize>,
        #[case] ambiguous: Vec<usize>,
        #[case] censored: Vec<usize>,
    ) {
        let result = align_prefix(left, right).unwrap();
        assert_eq!(result.pairs, pairs);
        assert_eq!(result.missing_left, missing);
        assert_eq!(result.ambiguous_left, ambiguous);
        assert_eq!(result.censored_left, censored);
    }

    proptest! {
        #[test]
        fn prefix_matches_exhaustive_endpoints(left in prop::collection::vec(0_u8..3,0..5),right in prop::collection::vec(0_u8..3,0..5)) {
            let mut alternatives=Vec::new();
            for end in 0..=left.len() {
                let prefix:Vec<_>=left.iter().take(end).copied().collect();
                for pairs in matchings(&prefix,&right,0,0,Vec::new()) {
                    alternatives.push((end+right.len()-2*pairs.len(),end,pairs));
                }
            }
            let cost=alternatives.iter().map(|(cost,_,_)|*cost).min().unwrap();
            let optimal:Vec<_>=alternatives.iter().filter(|(value,_,_)|*value==cost).collect();
            let ends:BTreeSet<_>=optimal.iter().map(|(_,end,_)|*end).collect();
            let mut forced:BTreeSet<_>=optimal.first().unwrap().2.iter().copied().collect();
            for (_,_,path) in &optimal {
                let pairs:BTreeSet<_>=path.iter().copied().collect();
                forced=forced.intersection(&pairs).copied().collect();
            }
            let first=*ends.first().unwrap();
            let last=*ends.last().unwrap();
            let missing:Vec<_>=(0..first).filter(|i|optimal.iter().all(|(_,_,path)|path.iter().all(|(a,_)|a!=i))).collect();
            let result=align_prefix(&left,&right).unwrap();
            prop_assert_eq!(result.cost as usize,cost);
            prop_assert_eq!(result.ends,ends.into_iter().collect::<Vec<_>>());
            prop_assert_eq!(result.pairs.into_iter().collect::<BTreeSet<_>>(),forced);
            prop_assert_eq!(result.missing_left,missing);
            prop_assert_eq!(result.censored_left,(last..left.len()).collect::<Vec<_>>());
        }
    }
}

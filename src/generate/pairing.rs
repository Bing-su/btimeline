#[derive(Debug)]
#[cfg_attr(test, derive(Clone, PartialEq, Eq))]
pub(super) struct PendingStart {
    pub(super) actor: i64,
    pub(super) instance: Option<i64>,
    pub(super) ability: i64,
    pub(super) event_index: usize,
    pub(super) timestamp: i64,
    pub(super) row: usize,
}

// Consume the last eligible start in list order, e.g. a completion at 20 cannot consume one at 30.
pub(super) fn start_for(
    starts: &mut Vec<PendingStart>,
    actor: i64,
    instance: Option<i64>,
    ability: i64,
    at: i64,
) -> Option<PendingStart> {
    let position = starts.iter().rposition(|start| {
        start.actor == actor
            && start.instance == instance
            && start.ability == ability
            && start.timestamp <= at
    })?;
    Some(starts.remove(position))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::empty(vec![], 20, None)]
    #[case::before(vec![10], 20, Some(0))]
    #[case::equal(vec![20], 20, Some(0))]
    #[case::future(vec![30], 20, None)]
    #[case::all_future(vec![30, 40], 20, None)]
    #[case::latest(vec![10, 20, 30], 20, Some(1))]
    #[case::equal_times(vec![20, 20], 20, Some(1))]
    #[case::list_order(vec![20, 10], 20, Some(1))]
    #[case::minimum(vec![i64::MIN], i64::MIN, Some(0))]
    #[case::maximum(vec![i64::MAX], i64::MAX, Some(0))]
    #[case::extreme_future(vec![i64::MAX], i64::MIN, None)]
    fn start_for_preserves_unselected_rows(
        #[case] timestamps: Vec<i64>,
        #[case] at: i64,
        #[case] selected: Option<usize>,
    ) {
        let mut starts: Vec<_> = timestamps
            .into_iter()
            .enumerate()
            .map(|(index, timestamp)| PendingStart {
                actor: 1,
                instance: None,
                ability: 2,
                event_index: index,
                timestamp,
                row: index + 100,
            })
            .collect();
        let original = starts.clone();
        assert_eq!(
            start_for(&mut starts, 1, None, 2, at),
            selected.and_then(|index| original.get(index).cloned())
        );
        let remaining: Vec<_> = original
            .into_iter()
            .enumerate()
            .filter_map(|(index, start)| (Some(index) != selected).then_some(start))
            .collect();
        assert_eq!(starts, remaining);
    }

    // Check the former postcondition on generated lists, e.g. interleaved identities and future starts.
    proptest! {
        #[test]
        fn start_for_matches_last_eligible_row_and_removes_only_it(
            rows in prop::collection::vec(
                (0i64..3, prop::option::of(0i64..3), 0i64..3, -2i64..=2),
                0..64,
            ),
            actor in 0i64..3,
            instance in prop::option::of(0i64..3),
            ability in 0i64..3,
            at in -2i64..=2,
        ) {
            let mut starts: Vec<_> = rows.into_iter().enumerate().map(
                |(index, (actor, instance, ability, timestamp))| PendingStart {
                    actor, instance, ability, timestamp,
                    event_index: index, row: index + 100,
                }
            ).collect();
            let original = starts.clone();
            let mut selected = None;
            for (index, start) in original.iter().enumerate() {
                if (start.actor, start.instance, start.ability) == (actor, instance, ability)
                    && start.timestamp <= at
                {
                    selected = Some(index);
                }
            }
            let result = start_for(&mut starts, actor, instance, ability, at);
            prop_assert_eq!(result, selected.and_then(|index| original.get(index).cloned()));
            let remaining: Vec<_> = original.into_iter().enumerate().filter_map(
                |(index, start)| (Some(index) != selected).then_some(start)
            ).collect();
            prop_assert_eq!(starts, remaining);
        }
    }
}

use vstd::prelude::*;

verus! {
#[derive(Debug)]
pub(super) struct PendingStart {
    pub(super) actor: i64,
    pub(super) instance: Option<i64>,
    pub(super) ability: i64,
    pub(super) event_index: usize,
    pub(super) timestamp: i64,
    pub(super) row: usize,
}

pub(super) closed spec fn matches(start: PendingStart, actor: i64, instance: Option<i64>, ability: i64, at: i64) -> bool {
    start.actor == actor && start.instance == instance && start.ability == ability && start.timestamp <= at
}

#[expect(clippy::indexing_slicing, reason = "The Verus loop invariant proves the index is in bounds.")]
fn matching_start_index(
    starts: &[PendingStart], actor: i64, instance: Option<i64>, ability: i64, at: i64,
) -> (result: Option<usize>)
    ensures
        match result {
            Some(i) => i < starts.len() && matches(starts@[i as int], actor, instance, ability, at)
                && forall|j: int| i < j && j < starts.len() ==> !matches(starts@[j], actor, instance, ability, at),
            None => forall|j: int| 0 <= j && j < starts.len() ==> !matches(starts@[j], actor, instance, ability, at),
        }
{
    let mut position = starts.len();
    while position > 0
        invariant
            position <= starts.len(),
            forall|j: int| position <= j && j < starts.len() ==> !matches(starts@[j], actor, instance, ability, at),
        decreases position,
    {
        position -= 1;
        let start = &starts[position];
        if start.actor == actor && start.instance == instance && start.ability == ability && start.timestamp <= at {
            return Some(position);
        }
    }
    None
}

// A completion at 20 consumes the latest matching start at or before 20, never one at 30.
#[expect(clippy::manual_map, reason = "Verus verifies the explicit removal branch.")]
pub(super) fn start_for(starts: &mut Vec<PendingStart>, actor: i64, instance: Option<i64>, ability: i64, at: i64) -> (result: Option<PendingStart>)
    ensures
        match result {
            Some(start) => exists|i: int| 0 <= i && i < old(starts).len()
                && start == old(starts)@[i]
                && matches(start, actor, instance, ability, at)
                && (forall|j: int| i < j && j < old(starts).len() ==> !matches(old(starts)@[j], actor, instance, ability, at))
                && final(starts)@ == old(starts)@.remove(i),
            None => final(starts)@ == old(starts)@
                && (forall|j: int| 0 <= j && j < old(starts).len() ==> !matches(old(starts)@[j], actor, instance, ability, at)),
        }
{
    match matching_start_index(starts, actor, instance, ability, at) {
        Some(position) => Some(starts.remove(position)),
        None => None,
    }
}
}

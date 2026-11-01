use anyhow::{Context, Result};
use kdl::KdlNode;

use super::{Batch, argument, prop};

#[derive(Debug, PartialEq, Eq)]
struct Repeat {
    start: usize,
    period: usize,
    iterations: usize,
}

// Prefer the shortest exact body and consume its full run; partial suffixes remain raw evidence.
// shortcut: greedy discovery omits overlapping alternatives; revisit when resolving competing loop bodies.
fn find<T: Eq>(sequence: &[T]) -> Vec<Repeat> {
    let mut rest = sequence;
    let mut offset = 0;
    let mut repeats = Vec::new();
    while rest.len() >= 2 {
        let repeated = (1..=rest.len() / 2).find_map(|period| {
            let body = rest.get(..period)?;
            let iterations = rest
                .chunks_exact(period)
                .take_while(|chunk| *chunk == body)
                .count();
            (iterations >= 2).then_some((period, iterations))
        });
        let consumed = if let Some((period, iterations)) = repeated {
            repeats.push(Repeat {
                start: offset,
                period,
                iterations,
            });
            period * iterations
        } else {
            1
        };
        rest = rest.split_at(consumed).1;
        offset += consumed;
    }
    repeats
}

pub(super) fn nodes(
    batches: &[Batch],
    phases: &[crate::generate::PhaseTransition],
) -> Result<Vec<KdlNode>> {
    // Compare unordered signal multisets, not packet order or exact cycle duration.
    let sequence: Vec<_> = batches
        .iter()
        .map(|batch| batch.signals.as_slice())
        .collect();
    let ends = phases
        .iter()
        .map(|phase| batches.partition_point(|batch| batch.at_ms < phase.relative_ms))
        .chain(std::iter::once(batches.len()));
    let mut start = 0;
    let mut candidates = Vec::new();
    // Reported phase boundaries constrain discovery without becoming execution signals.
    for end in ends {
        let region = sequence
            .get(start..end)
            .context("Invalid repeat phase range")?;
        candidates.extend(find(region).into_iter().map(|mut repeat| {
            repeat.start += start;
            repeat
        }));
        start = end;
    }
    candidates
        .into_iter()
        .map(|repeat| {
            let mut node = KdlNode::new("repeat-candidate");
            prop(&mut node, "first_batch", i128::try_from(repeat.start)?);
            prop(&mut node, "period_batches", i128::try_from(repeat.period)?);
            prop(
                &mut node,
                "observed_iterations",
                i128::try_from(repeat.iterations)?,
            );
            for iteration in 0..repeat.iterations {
                let index = repeat.start + iteration * repeat.period;
                let batch = batches.get(index).context("Missing repeat boundary")?;
                let mut sample = KdlNode::new("iteration");
                argument(&mut sample, i128::try_from(iteration + 1)?);
                prop(&mut sample, "first_batch", i128::try_from(index)?);
                prop(&mut sample, "at_ms", i128::from(batch.at_ms));
                node.ensure_children().nodes_mut().push(sample);
            }
            Ok(node)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[rstest::rstest]
    #[case(b"", vec![])]
    #[case(b"ABC", vec![])]
    #[case(b"AAAA", vec![Repeat {start:0,period:1,iterations:4}])]
    #[case(b"XABABABAY", vec![Repeat {start:1,period:2,iterations:3}])]
    #[case(b"ABCABCXYZXYZ", vec![Repeat {start:0,period:3,iterations:2},Repeat {start:6,period:3,iterations:2}])]
    fn finds_exact_observed_runs(#[case] sequence: &[u8], #[case] expected: Vec<Repeat>) {
        assert_eq!(find(sequence), expected);
    }

    proptest! {
        #[test]
        fn every_candidate_is_exact_and_non_overlapping(sequence in prop::collection::vec(0_u8..5, 0..80)) {
            let mut previous_end = 0;
            for repeat in find(&sequence) {
                prop_assert!(repeat.start >= previous_end);
                prop_assert!(repeat.period > 0 && repeat.iterations >= 2);
                let end = repeat.start + repeat.period * repeat.iterations;
                let region = sequence.get(repeat.start..end).ok_or_else(|| TestCaseError::fail("repeat outside evidence"))?;
                let body = region.get(..repeat.period).ok_or_else(|| TestCaseError::fail("body outside repeat"))?;
                prop_assert!(region.chunks_exact(repeat.period).all(|chunk| chunk == body));
                previous_end = end;
            }
        }
    }
}

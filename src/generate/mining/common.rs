use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use kdl::KdlNode;

use super::{Pull, argument, prop};

type Timings<K = String> = BTreeMap<K, Vec<i64>>;

// Match final display precision without rounding evidence, e.g. 1049 and 1050 ms stay unordered.
const ORDER_RESOLUTION_MS: i64 = 100;

fn relation(left: i64, right: i64) -> std::cmp::Ordering {
    if left.abs_diff(right) <= ORDER_RESOLUTION_MS as u64 {
        std::cmp::Ordering::Equal
    } else {
        left.cmp(&right)
    }
}

// Match unique occurrences only; missing an earlier repeated cast must not shift every later match.
pub(super) fn anchors<K: Ord + Clone>(tables: &[Timings<K>]) -> Timings<K> {
    let Some(first) = tables.first().filter(|_| tables.len() >= 2) else {
        return Timings::new();
    };
    first
        .keys()
        .filter_map(|key| {
            let times = tables
                .iter()
                .map(|table| {
                    let values = table.get(key)?;
                    match values.as_slice() {
                        [at] => Some(*at),
                        _ => None,
                    }
                })
                .collect::<Option<Vec<_>>>()?;
            Some((key.clone(), times))
        })
        .collect()
}

pub(super) fn consistent_order<K: Ord>(common: &Timings<K>) -> bool {
    common.values().enumerate().all(|(index, times)| {
        common.values().skip(index + 1).all(|other| {
            let mut pairs = times.iter().zip(other);
            let Some((left, right)) = pairs.next() else {
                return true;
            };
            let first = relation(*left, *right);
            pairs.all(|(left, right)| relation(*left, *right) == first)
        })
    })
}

fn timings(pull: &Pull) -> Result<Timings> {
    let mut table = Timings::new();
    for batch in &pull.batches {
        for signal in &batch.signals {
            table
                .entry(serde_json::to_string(signal)?)
                .or_default()
                .push(batch.at_ms);
        }
    }
    Ok(table)
}

// Intersect observed orders, then omit transitive edges; A->B->C needs no extra A->C.
fn cover_relations(common: &Timings) -> Vec<(&str, &str)> {
    let successors: BTreeMap<_, BTreeSet<_>> = common
        .iter()
        .map(|(from, times)| {
            let after = common
                .iter()
                .filter(|(_, other)| {
                    times
                        .iter()
                        .zip(other.iter())
                        .all(|(left, right)| relation(*left, *right).is_lt())
                })
                .map(|(key, _)| key.as_str())
                .collect();
            (from.as_str(), after)
        })
        .collect();
    successors
        .iter()
        .flat_map(|(from, targets)| {
            targets
                .iter()
                .filter(|to| {
                    !targets.iter().any(|middle| {
                        successors
                            .get(middle)
                            .is_some_and(|after| after.contains(*to))
                    })
                })
                .map(|to| (*from, *to))
        })
        .collect()
}

fn anchor_node(
    key: &str,
    times: &[i64],
    completed: &[&Pull],
    ids: &BTreeMap<String, String>,
) -> Result<KdlNode> {
    let mut anchor = KdlNode::new("anchor");
    argument(
        &mut anchor,
        ids.get(key).context("Missing common activity")?.as_str(),
    );
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    prop(
        &mut anchor,
        "min_ms",
        i128::from(*sorted.first().context("Empty anchor")?),
    );
    prop(
        &mut anchor,
        "median_ms",
        i128::from(*sorted.get((sorted.len() - 1) / 2).context("Empty anchor")?),
    );
    prop(
        &mut anchor,
        "max_ms",
        i128::from(*sorted.last().context("Empty anchor")?),
    );
    for (pull, at) in completed.iter().zip(times) {
        let mut sample = KdlNode::new("sample");
        argument(&mut sample, format!("{}/{}", pull.report, pull.fight));
        prop(&mut sample, "at_ms", i128::from(*at));
        anchor.ensure_children().nodes_mut().push(sample);
    }
    Ok(anchor)
}

pub(super) fn node(pulls: &[Pull], ids: &BTreeMap<String, String>) -> Result<KdlNode> {
    // Wipes are censored evidence, not completed traces that disprove an unobserved suffix.
    let completed: Vec<_> = pulls.iter().filter(|pull| pull.kill).collect();
    let tables = completed
        .iter()
        .map(|pull| timings(pull))
        .collect::<Result<Vec<_>>>()?;
    let common = anchors(&tables);
    let consistent = consistent_order(&common);
    let status = match (tables.len(), common.is_empty(), consistent) {
        (0 | 1, _, _) => "insufficient-completed-pulls",
        (_, true, _) => "no-unique-anchors",
        (_, false, false) => "ambiguous-order",
        _ => "observed-common-order",
    };
    let mut node = KdlNode::new("common-progress");
    prop(&mut node, "status", status);
    prop(
        &mut node,
        "order_resolution_ms",
        i128::from(ORDER_RESOLUTION_MS),
    );
    prop(
        &mut node,
        "completed_pulls",
        i128::try_from(completed.len())?,
    );
    for (key, times) in &common {
        node.ensure_children()
            .nodes_mut()
            .push(anchor_node(key, times, &completed, ids)?);
    }
    for (from, to) in cover_relations(&common) {
        let mut relation = KdlNode::new("precedes");
        argument(
            &mut relation,
            ids.get(from).context("Missing common activity")?.as_str(),
        );
        argument(
            &mut relation,
            ids.get(to).context("Missing common activity")?.as_str(),
        );
        node.ensure_children().nodes_mut().push(relation);
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn table(entries: &[(&str, &[i64])]) -> Timings {
        entries
            .iter()
            .map(|(key, times)| ((*key).into(), times.to_vec()))
            .collect()
    }

    #[test]
    fn repeated_missing_and_optional_signals_do_not_become_anchors() {
        let first = table(&[("A", &[100]), ("B", &[200, 300]), ("C", &[400])]);
        let second = table(&[("A", &[101]), ("B", &[301]), ("D", &[500])]);
        assert_eq!(anchors(&[first, second]), table(&[("A", &[100, 101])]));
    }

    #[test]
    fn a_single_pull_does_not_establish_common_progress() {
        assert!(anchors(&[table(&[("A", &[100])])]).is_empty());
    }

    #[test]
    fn order_changes_are_distinct_from_packet_jitter() {
        assert!(consistent_order(&table(&[
            ("A", &[1049, 1050]),
            ("B", &[1050, 1049])
        ])));
        assert!(!consistent_order(&table(&[
            ("A", &[100, 400]),
            ("B", &[400, 100])
        ])));
    }

    #[rstest::rstest]
    #[case(table(&[("A", &[0,0]),("B", &[1000,1000]),("C", &[2000,2000])]), vec![("A","B"),("B","C")])]
    #[case(table(&[("A", &[0,1000]),("B", &[1000,0]),("C", &[2000,2000])]), vec![("A","C"),("B","C")])]
    #[case(table(&[("A", &[0,0]),("B", &[0,1]),("C", &[2000,2000])]), vec![("A","C"),("B","C")])]
    #[case(table(&[("A", &[0,0]),("B", &[80,80]),("C", &[160,160])]), vec![("A","C")])]
    #[case(table(&[("A", &[0,0]),("B", &[100,100]),("C", &[101,101])]), vec![("A","C")])]
    #[case(table(&[("A", &[0,0]),("B", &[0,0])]), vec![])]
    fn common_edges_preserve_only_agreed_order(
        #[case] common: Timings,
        #[case] expected: Vec<(&str, &str)>,
    ) {
        assert_eq!(cover_relations(&common), expected);
    }

    fn reachable(edges: &[(&str, &str)], from: &str, to: &str) -> bool {
        let mut pending: Vec<_> = edges
            .iter()
            .filter(|(left, _)| *left == from)
            .map(|(_, right)| *right)
            .collect();
        let mut visited = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if current == to {
                return true;
            }
            if !visited.insert(current) {
                continue;
            }
            pending.extend(
                edges
                    .iter()
                    .filter(|(left, _)| *left == current)
                    .map(|(_, right)| *right),
            );
        }
        false
    }

    proptest! {
        #[test]
        fn reduction_preserves_exactly_the_observed_partial_order(
            samples in prop::collection::vec((0_i64..1000, 0_i64..1000), 0..12)
        ) {
            let common: Timings = samples.iter().enumerate().map(|(index, (first, second))| {
                (format!("k{index}"), vec![*first, *second])
            }).collect();
            let edges = cover_relations(&common);
            for (from, times) in &common {
                for (to, other) in &common {
                    let observed = times.iter().zip(other).all(|(left, right)| relation(*left, *right).is_lt());
                    prop_assert_eq!(reachable(&edges, from, to), observed);
                }
            }
            let reversed: Timings = common.iter().map(|(key, times)| {
                (key.clone(), times.iter().rev().copied().collect())
            }).collect();
            prop_assert_eq!(cover_relations(&common), cover_relations(&reversed));
        }
    }
}

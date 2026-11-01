use std::collections::BTreeMap;

use anyhow::{Context, Result};
use kdl::KdlNode;

use super::{Pull, argument, prop};

type Timings = BTreeMap<String, Vec<i64>>;

// Match unique occurrences only; missing an earlier repeated cast must not shift every later match.
fn anchors(tables: &[Timings]) -> Timings {
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

fn order(anchors: &Timings, trace: usize) -> Vec<Vec<&str>> {
    let mut batches = BTreeMap::<i64, Vec<&str>>::new();
    for (key, times) in anchors {
        if let Some(at) = times.get(trace) {
            batches.entry(*at).or_default().push(key);
        }
    }
    batches.into_values().collect()
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

pub(super) fn node(pulls: &[Pull], ids: &BTreeMap<String, String>) -> Result<KdlNode> {
    // Wipes are censored evidence, not completed traces that disprove an unobserved suffix.
    let completed: Vec<_> = pulls.iter().filter(|pull| pull.kill).collect();
    let tables = completed
        .iter()
        .map(|pull| timings(pull))
        .collect::<Result<Vec<_>>>()?;
    let common = anchors(&tables);
    let sequence = order(&common, 0);
    let consistent = (1..tables.len()).all(|index| order(&common, index) == sequence);
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
        "completed_pulls",
        i128::try_from(completed.len())?,
    );
    for (key, times) in &common {
        let mut anchor = KdlNode::new("anchor");
        argument(
            &mut anchor,
            ids.get(key).context("Missing common activity")?.as_str(),
        );
        let mut sorted = times.clone();
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
        node.ensure_children().nodes_mut().push(anchor);
    }
    if consistent {
        for group in sequence {
            let mut step = KdlNode::new("step");
            for key in group {
                argument(
                    &mut step,
                    ids.get(key).context("Missing common activity")?.as_str(),
                );
            }
            node.ensure_children().nodes_mut().push(step);
        }
    }
    Ok(node)
}

#[cfg(test)]
mod tests {
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
    fn order_changes_and_simultaneity_are_visible() {
        let common = table(&[("A", &[100, 200, 100]), ("B", &[200, 100, 100])]);
        assert_eq!(order(&common, 0), vec![vec!["A"], vec!["B"]]);
        assert_eq!(order(&common, 1), vec![vec!["B"], vec!["A"]]);
        assert_eq!(order(&common, 2), vec![vec!["A", "B"]]);
    }
}

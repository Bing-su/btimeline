use std::collections::BTreeSet;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::super::{Candidate, Progression, Route, choices, common, prefixes};
use super::Repeat;

fn cycles(
    repeat: &Repeat,
    complete: &[Candidate],
    training: &Progression,
) -> Result<Vec<Candidate>> {
    let width = repeat.body.len();
    let mut result = Vec::new();
    for (trace, waves) in &repeat.observed_waves {
        let Some(reference) = complete
            .iter()
            .find(|candidate| &candidate.reference == trace)
        else {
            continue;
        };
        let times = training
            .samples
            .get(trace)
            .context("Missing repeat timestamps")?;
        for start in (0..waves.len()).step_by(width) {
            if waves.len() - start < width {
                break;
            }
            // Include the next boundary when observed, e.g. a complete last cycle still has a recurrence deadline.
            let nodes = waves
                .iter()
                .skip(start)
                .take(width + 1)
                .flatten()
                .map(|node| Ok((*node, *times.get(*node).context("Missing cycle timestamp")?)))
                .collect::<Result<_>>()?;
            result.push(choices::slice(reference, nodes));
        }
    }
    Ok(result)
}

// Count reports' traces once, e.g. ten cycles from one long log are not ten independent supporters.
fn support(
    peer: &Candidate,
    observed: &Progression,
    complete: &[Candidate],
    training: &Progression,
) -> Result<(usize, Option<i64>)> {
    let mut traces = BTreeSet::new();
    let mut deadline = None;
    for reference in complete {
        if peer.same_prefix(reference, observed, training)? {
            traces.insert(&reference.reference);
            if let Some(next) = peer.next_expected_at(reference) {
                deadline = Some(deadline.map_or(next, |at: i64| at.min(next)));
            }
        }
    }
    Ok((traces.len(), deadline))
}

fn uncertain(nodes: &[(usize, i64)]) -> bool {
    nodes.windows(2).any(|pair| {
        pair.first()
            .zip(pair.get(1))
            .is_some_and(|((_, a), (_, b))| *b <= a.saturating_add(common::ORDER_RESOLUTION_MS))
    })
}

impl Repeat {
    pub(in crate::generate::mining::progression::draft) fn check(
        &self,
        route: &Route,
        peer: &Candidate,
        complete: &[Candidate],
        observed: &Progression,
        training: &Progression,
    ) -> Result<Value> {
        let mut row = json!({"trace":peer.reference,"source":route.source,"training":false,
            "method":"observed-repeat-cycles-v1","status":"unresolved","scope_end_ms":peer.end_ms,
            "supporting_traces":0,"next_expected_ms":null,"complete_cycles":0,
            "reason":"incomplete finite model or unsupported compound repeat waves"});
        // shortcut: validate singleton waves in observed orders; extend with partial-order and fragment conformance.
        if !route.peers_complete
            || complete.is_empty()
            || self
                .observed_waves
                .values()
                .flatten()
                .any(|wave| wave.len() != 1)
        {
            return Ok(row);
        }
        let prefix_len = complete
            .first()
            .context("Missing repeat opener")?
            .nodes
            .len();
        let opening = choices::slice(peer, peer.nodes.iter().take(prefix_len).copied().collect());
        let (prefix_support, prefix_deadline) = prefixes::support(
            &opening,
            observed,
            complete,
            training,
            route.choices.first(),
        )?;
        let fields = row.as_object_mut().context("Invalid repeat check")?;
        fields.insert("prefix_supporting_traces".into(), prefix_support.into());
        if peer.nodes.len() < prefix_len {
            fields.insert("supporting_traces".into(), prefix_support.into());
            fields.insert("next_expected_ms".into(), prefix_deadline.into());
            let censored = prefix_support > 0 && prefix_deadline.is_some_and(|at| peer.end_ms < at);
            fields.insert(
                "status".into(),
                if censored { "censored" } else { "unresolved" }.into(),
            );
            fields.insert(
                "reason".into(),
                "finite prefix before repeat entry; next-occurrence deadline checked".into(),
            );
            return Ok(row);
        }
        if prefix_support == 0 {
            fields.insert(
                "reason".into(),
                "finite opener differs or its choice order is uncertain".into(),
            );
            return Ok(row);
        }
        let tail = peer
            .nodes
            .get(prefix_len - 1..)
            .context("Missing repeat entry")?;
        if uncertain(tail) {
            fields.insert(
                "reason".into(),
                "repeat occurrence order is uncertain within 100ms".into(),
            );
            return Ok(row);
        }
        let references = cycles(self, complete, training)?;
        let width = self.body.len();
        let mut next_expected = None;
        for (index, chunk) in tail.chunks(width).enumerate() {
            let current = choices::slice(peer, chunk.to_vec());
            let (matching, deadline) = support(&current, observed, &references, training)?;
            next_expected = deadline;
            fields.insert("supporting_traces".into(), matching.into());
            fields.insert("next_expected_ms".into(), deadline.into());
            if matching == 0 {
                fields.insert(
                    "reason".into(),
                    "unseen cycle order, missing step or misplaced repeat boundary".into(),
                );
                fields.insert("failed_cycle".into(), (index + 1).into());
                return Ok(row);
            }
            fields.insert(
                "complete_cycles".into(),
                (index + usize::from(chunk.len() == width)).into(),
            );
        }
        let censored = next_expected.is_some_and(|at| peer.end_ms < at);
        fields.insert(
            "status".into(),
            if censored { "censored" } else { "unresolved" }.into(),
        );
        fields.insert(
            "reason".into(),
            if censored {
                "observed cycles match; scope ended before the next occurrence"
            } else {
                "next repeat occurrence is overdue or lacks an observed training gap"
            }
            .into(),
        );
        Ok(row)
    }
}

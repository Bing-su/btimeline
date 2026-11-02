use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Candidate, Progression, Route, choices};

fn ambiguous_choice(
    peer: &Candidate,
    progression: &Progression,
    choice: &choices::Choice,
) -> Result<bool> {
    let Some((_, at)) = peer.nodes.get(choice.join) else {
        return Ok(false);
    };
    let path = choices::activities(peer, progression)?;
    let trigger = path.get(choice.join).context("Missing prefix selection")?;
    let boundary = at.saturating_add(super::common::ORDER_RESOLUTION_MS);
    // Packet jitter cannot establish which competing choice came first, e.g. B/C only 80ms apart.
    Ok(path
        .iter()
        .zip(&peer.nodes)
        .skip(choice.join + 1)
        .take_while(|(_, (_, at))| *at <= boundary)
        .any(|(signal, _)| {
            signal != trigger
                && choice.options.iter().any(|option| {
                    option
                        .graph
                        .node_weights()
                        .next()
                        .is_some_and(|node| &node.activity == signal)
                })
        }))
}

// Use only matching paths, e.g. a fast right option cannot set the left option's missing-event deadline.
pub(super) fn support(
    peer: &Candidate,
    observed: &Progression,
    complete: &[Candidate],
    training: &Progression,
    choice: Option<&choices::Choice>,
) -> Result<(usize, Option<i64>)> {
    if let Some(choice) = choice
        && ambiguous_choice(peer, observed, choice)?
    {
        return Ok((0, None));
    }
    let mut matching = 0;
    let mut next_expected = None;
    for path in complete {
        if peer.same_prefix(path, observed, training)? {
            matching += 1;
            if let Some(next) = peer.next_expected_at(path) {
                next_expected = Some(next_expected.map_or(next, |current: i64| current.min(next)));
            }
        }
    }
    Ok((matching, next_expected))
}

pub(super) fn bind(
    route: &mut Route,
    peer: &Candidate,
    complete: &[Candidate],
    progression: &Progression,
) -> Result<Value> {
    let choice = route.choices.first().context("Missing learned choice")?;
    let prefix = choice.join;
    let body = choice
        .options
        .first()
        .context("Missing choice option")?
        .graph
        .node_count();
    let (matching, next_expected_ms) =
        support(peer, progression, complete, progression, Some(choice))?;
    let censored = matching > 0 && next_expected_ms.is_some_and(|at| peer.end_ms < at);
    let selected = if censored && peer.nodes.len() > prefix {
        let path = choices::activities(peer, progression)?;
        Some(path.get(prefix).context("Missing observed choice")?.clone())
    } else {
        None
    };
    if censored {
        // The backbone omits the option body; bind its prefix and only the observed post-join suffix.
        let common = peer
            .nodes
            .iter()
            .take(prefix)
            .chain(peer.nodes.iter().skip(prefix + body));
        for (node, (observation, _)) in route.graph.node_weights_mut().zip(common) {
            node.observations
                .insert(peer.reference.clone(), *observation);
        }
        if let Some(trigger) = &selected {
            let option = route
                .choices
                .first_mut()
                .context("Missing learned choice")?
                .options
                .iter_mut()
                .find(|option| {
                    option
                        .graph
                        .node_weights()
                        .next()
                        .is_some_and(|node| &node.activity == trigger)
                })
                .context("Observed trigger has no learned option")?;
            for (node, (observation, _)) in option
                .graph
                .node_weights_mut()
                .zip(peer.nodes.iter().skip(prefix).take(body))
            {
                node.observations
                    .insert(peer.reference.clone(), *observation);
            }
            option.refresh_relations(progression)?;
        }
    } else {
        route.peers_complete = false;
    }
    Ok(
        json!({"method":"representative-choice-prefix-v2","trace":peer.reference,"reference":route.reference,
        "status":if censored {"censored"} else {"unresolved"},"scope_end_ms":peer.end_ms,"next_expected_ms":next_expected_ms,
        "selection":if selected.is_some() {"observed"} else {"unobserved"},"option_trigger":selected,"supporting_traces":matching,
        "reason":if censored {"observed prefix ended before the next occurrence on every matching path"} else {"prefix differs, choice order is uncertain or the next occurrence is due within the observed scope"}}),
    )
}

use anyhow::{Context, Result};
use serde_json::json;

use super::{Common, Group, Pull, Signal, alignment, interval, trace};

struct Scope<'a> {
    pull: &'a Pull,
    at: i64,
    end_ms: i64,
    groups: Vec<Group<'a>>,
}

fn scope<'a>(
    pull: &'a Pull,
    anchor: &Signal,
    earlier: &[&Signal],
    include: &impl Fn(&Signal) -> bool,
) -> Result<Scope<'a>, &'static str> {
    let table = super::super::timing_table(pull);
    let Some([at]) = table.get(anchor).map(Vec::as_slice) else {
        return Err("no uniquely observed tail anchor");
    };
    if earlier.iter().any(|signal| {
        table.get(*signal).is_some_and(|times| {
            times
                .iter()
                .any(|time| *time > *at + super::super::common::ORDER_RESOLUTION_MS)
        })
    }) {
        return Err("common anchor order contradicts tail origin");
    }
    Ok(Scope {
        pull,
        at: *at,
        end_ms: pull.scope_end(&anchor.source),
        groups: interval(
            pull,
            *at,
            None,
            &anchor.source,
            include,
            super::Boundary::Ordered,
        )?,
    })
}

fn align(reference: &[Group<'_>], peer: &[Group<'_>]) -> Result<alignment::AttemptAlignment> {
    alignment::align_prefix(
        &reference.iter().map(|group| &group.key).collect::<Vec<_>>(),
        &peer.iter().map(|group| &group.key).collect::<Vec<_>>(),
    )
}

// A missing suffix is neutral only after both every optimal endpoint and every supported time.
fn supported(
    index: usize,
    scopes: &[Scope<'_>],
    matches: &[alignment::AttemptAlignment],
) -> Option<Vec<(usize, usize)>> {
    let peers: Vec<_> = matches
        .iter()
        .enumerate()
        .filter_map(|(peer, aligned)| {
            aligned
                .pairs
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, j)| (peer, *j))
        })
        .collect();
    if peers.len() < 2 {
        return None;
    }
    let earliest = peers
        .iter()
        .filter_map(|(i, j)| {
            let scope = scopes.get(*i)?;
            scope
                .groups
                .get(*j)?
                .observations
                .iter()
                .map(|row| row.at - scope.at)
                .min()
        })
        .min()?;
    let valid = scopes
        .iter()
        .zip(matches)
        .enumerate()
        .all(|(peer, (scope, aligned))| {
            peers.iter().any(|(i, _)| *i == peer)
                || (aligned.censored_left.contains(&index) && scope.at + earliest > scope.end_ms)
        });
    valid.then_some(peers)
}

fn learn(scopes: &[Scope<'_>], anchor: &Signal, result: &mut Common) -> Result<()> {
    let reference = scopes.first().context("Missing tail reference")?;
    if scopes
        .iter()
        .any(|scope| !alignment::within_limit(reference.groups.len(), scope.groups.len()))
    {
        result
            .diagnostics
            .push(json!({"method":"censored-tail-v1","anchor":anchor,"status":"resource-limit"}));
        return Ok(());
    }
    let matches = scopes
        .iter()
        .map(|scope| align(&reference.groups, &scope.groups))
        .collect::<Result<Vec<_>>>()?;
    let mut generated = Vec::new();
    let mut rejected = Vec::new();
    for index in 0..reference.groups.len() {
        let Some(peers) = supported(index, scopes, &matches) else {
            rejected.push(json!({"reference_batches":super::batches(&reference.groups,&[index]),"reason":"requires two forced matches and genuinely censored non-supporting traces"}));
            continue;
        };
        let pulls = peers
            .iter()
            .map(|(i, _)| {
                scopes
                    .get(*i)
                    .map(|scope| scope.pull)
                    .context("Missing tail trace")
            })
            .collect::<Result<Vec<_>>>()?;
        let groups = peers
            .iter()
            .map(|(i, j)| {
                scopes
                    .get(*i)
                    .and_then(|scope| scope.groups.get(*j))
                    .context("Missing tail group")
            })
            .collect::<Result<Vec<_>>>()?;
        let bounds = json!({"left":anchor,"right":null});
        let events = super::events(&pulls, &groups, &bounds)?;
        if events.is_empty() {
            rejected.push(json!({"reference_batches":super::batches(&reference.groups,&[index]),"reason":"repeated identity inside uncertainty group"}));
        }
        for mut event in events {
            let evidence = event
                .evidence
                .as_object_mut()
                .context("Invalid tail evidence")?;
            evidence.insert("method".into(), "censored-tail-v1".into());
            evidence.insert("sync".into(), "omitted: partial-trace display event".into());
            generated.push(json!({"activity":event.signal,"samples":event.samples}));
            result.events.push(event);
        }
    }
    let checks: Vec<_> = scopes
        .iter()
        .zip(&matches)
        .map(|(scope, aligned)| {
            json!({
                "trace":trace(scope.pull),"kill":scope.pull.kill,"anchor_at_ms":scope.at,
                "edit_cost":aligned.cost,"reference_group_endpoints":aligned.ends,
                "ambiguous_reference":super::batches(&reference.groups,&aligned.ambiguous_left),
                "missing_reference":super::batches(&reference.groups,&aligned.missing_left),
                "censored_reference":super::batches(&reference.groups,&aligned.censored_left),
                "peer_only":super::batches(&scope.groups,&aligned.peer_only)
            })
        })
        .collect();
    result.diagnostics.push(
        json!({"method":"censored-tail-v1","anchor":anchor,"status":"evaluated",
        "reference":trace(reference.pull),"checks":checks,"generated":generated,"rejected":rejected}),
    );
    Ok(())
}

pub(super) fn expand(
    pulls: &[&Pull],
    anchors: &[(&Signal, &std::collections::BTreeMap<String, i64>)],
    include: &impl Fn(&Signal) -> bool,
    raid: bool,
    result: &mut Common,
) -> Result<()> {
    let Some((anchor, _)) = anchors.last() else {
        return Ok(());
    };
    let earlier: Vec<_> = anchors
        .iter()
        .map(|(signal, _)| *signal)
        .filter(|signal| *signal != *anchor)
        .collect();
    let mut scopes = Vec::new();
    let mut blocked = false;
    // Boss modes lack an independently known origin for new incomplete traces; keep their clears.
    for pull in pulls.iter().filter(|pull| raid || pull.kill) {
        match scope(pull, anchor, &earlier, include) {
            Ok(scope) => scopes.push(scope),
            Err(reason) => {
                blocked |= pull.kill;
                result.diagnostics.push(json!({"method":"censored-tail-v1","anchor":anchor,"trace":trace(pull),"status":"unresolved","reason":reason}));
            }
        }
    }
    if blocked || scopes.len() < 2 {
        return Ok(());
    }
    // Longest observed suffix supplies occurrence labels without assuming kill=true is longest.
    scopes.sort_by_key(|scope| (std::cmp::Reverse(scope.groups.len()), trace(scope.pull)));
    learn(&scopes, anchor, result)
}

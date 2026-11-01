use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::{
    Boundary,
    Common,
    Pull,
    Signal,
    expand_interval,
    fragments,
    interval,
    opening_start,
    trace,
};

fn learn(
    pulls: &[&Pull],
    source: &Option<String>,
    include: &impl Fn(&Signal) -> bool,
    result: &mut Common,
) -> Result<()> {
    let mut scopes = pulls
        .iter()
        .map(|pull| {
            let start = opening_start(pull, source, include, false).map_err(anyhow::Error::msg)?;
            let groups = interval(pull, start, None, source, include, Boundary::Display)
                .map_err(anyhow::Error::msg)?;
            Ok((*pull, groups))
        })
        .collect::<Result<Vec<_>>>()?;
    // A short completed path avoids treating a longer rotation's unobserved future as mandatory.
    scopes.sort_by_key(|(pull, groups)| (groups.len(), trace(pull)));
    let (pulls, mut rows): (Vec<_>, Vec<_>) = scopes.into_iter().unzip();
    fragments::normalize(&mut rows)?;
    let first = rows
        .first()
        .and_then(|row| row.first())
        .context("Missing boss entry")?;
    ensure!(
        first.key.len() == 1
            && rows
                .iter()
                .all(|row| row.first().is_some_and(|group| group.key == first.key)),
        "Boss entry groups do not identify the same single opening activity"
    );
    let bounds = json!({"left":null,"right":null,"origin":"boss-cast","boundary_policy":"display-interior-v1"});
    let groups = rows
        .iter()
        .filter_map(|row| row.first())
        .collect::<Vec<_>>();
    let mut opening = super::events(&pulls, &groups, &bounds)?;
    for event in &mut opening {
        event
            .evidence
            .as_object_mut()
            .context("Invalid entry evidence")?
            .insert("entry".into(), true.into());
    }
    let mut learned = Common {
        events: opening,
        diagnostics: Vec::new(),
        branches: Vec::new(),
    };
    let tails = rows
        .iter()
        .map(|row| row.get(1..).context("Missing boss suffix"))
        .collect::<Result<Vec<_>>>()?;
    expand_interval(&pulls, &tails, bounds, &mut learned)?;
    for event in &mut learned.events {
        event
            .evidence
            .as_object_mut()
            .context("Invalid entry evidence")?
            .insert("method".into(), "boss-entry-lcs-v1".into());
    }
    learned.diagnostics.push(json!({"method":"boss-entry-lcs-v1","source":source,"status":"evaluated",
        "reference":pulls.first().map(|pull|trace(pull)),"generated":learned.events.len(),
        "not_checked":["conditional paths and loops","unobserved longer suffixes","ACT signal equivalence"]}));
    result.events.extend(learned.events);
    result.diagnostics.extend(learned.diagnostics);
    Ok(())
}

// A known boss entry gives structural position even when every activity repeats later in the trace.
pub(super) fn expand(
    pulls: &[&Pull],
    unique: &BTreeMap<Signal, BTreeMap<String, i64>>,
    include: &impl Fn(&Signal) -> bool,
    result: &mut Common,
) -> Result<()> {
    let anchored: BTreeSet<_> = unique.keys().map(|signal| &signal.source).collect();
    let sources: BTreeSet<_> = pulls
        .iter()
        .flat_map(|pull| &pull.batches)
        .flat_map(|batch| batch.waves())
        .filter(|signal| include(signal) && !anchored.contains(&signal.source))
        .map(|signal| &signal.source)
        .collect();
    for source in sources {
        if let Err(error) = learn(pulls, source, include, result) {
            result.diagnostics.push(json!({"method":"boss-entry-lcs-v1","source":source,"status":"unresolved","reason":error.to_string()}));
        }
    }
    Ok(())
}

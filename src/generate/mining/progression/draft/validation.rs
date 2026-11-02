use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{
    Candidate,
    Draft,
    Progression,
    Pull,
    Route,
    Signal,
    choices,
    learning_views,
    prefixes,
};

fn paths(route: &Route) -> Result<Vec<Vec<Signal>>> {
    ensure!(
        route.choices.len() <= 1,
        "Validation does not support multiple independent choice regions"
    );
    let common: Vec<_> = route
        .graph
        .node_weights()
        .map(|node| node.activity.clone())
        .collect();
    let Some(choice) = route.choices.first() else {
        return Ok(vec![common]);
    };
    choice
        .options
        .iter()
        .map(|option| {
            ensure!(
                option.choices.is_empty(),
                "Validation does not support nested choices"
            );
            Ok(common
                .get(..choice.join)
                .context("Missing choice prefix")?
                .iter()
                .cloned()
                .chain(
                    option
                        .graph
                        .node_weights()
                        .map(|node| node.activity.clone()),
                )
                .chain(
                    common
                        .get(choice.join..)
                        .context("Missing choice suffix")?
                        .iter()
                        .cloned(),
                )
                .collect())
        })
        .collect()
}

// shortcut: check observed paths and terminated prefixes; add KDL timing validation with its own evidence.
pub(super) fn check(pulls: &[Pull], boss_only: bool) -> Result<Value> {
    let reports: BTreeSet<_> = pulls.iter().map(|pull| &pull.report).collect();
    let not_checked = [
        "final KDL execution and timing",
        "unobserved suffix conformance",
        "terminal repeat path conformance",
        "statistical independence",
        "ACT signal equivalence",
    ];
    if reports.len() < 3 {
        return Ok(
            json!({"method":"automatic-draft-validation-v1","status":"insufficient-reports",
            "reports":reports.len(),"required":3,"not_checked":not_checked}),
        );
    }
    let report = reports.last().context("Missing holdout report")?;
    let (mut withheld, mut training): (Vec<_>, Vec<_>) = pulls
        .iter()
        .cloned()
        .partition(|pull| &pull.report == *report);
    training.sort_by(|a, b| (&a.report, a.fight).cmp(&(&b.report, b.fight)));
    withheld.sort_by(|a, b| (&a.report, a.fight).cmp(&(&b.report, b.fight)));
    let trace = |pull: &Pull| format!("{}/{}", pull.report, pull.fight);
    let split = json!({"training":training.iter().map(trace).collect::<Vec<_>>(),"held_out":withheld.iter().map(trace).collect::<Vec<_>>()});
    let training_observations = Progression::new(&training);
    let model = Draft::new(&training, &training_observations, boss_only, &[])?;
    let (training_views, _) = learning_views(&training, boss_only);
    let (withheld_views, _) = learning_views(&withheld, boss_only);
    let observed = Progression::new(&withheld);
    // Check unseen bosses explicitly, e.g. a holdout-only final boss must not disappear or train the model.
    let sources: BTreeSet<_> = model
        .routes
        .iter()
        .map(|route| route.source.clone())
        .chain(
            withheld_views
                .iter()
                .flat_map(|pull| pull.boss_names.iter().cloned().map(Some))
                .filter(|_| boss_only),
        )
        .chain((!boss_only).then_some(None))
        .collect();
    let mut checks = Vec::new();
    for source in sources {
        let route = model.routes.iter().find(|route| route.source == source);
        let model_paths = route.map(paths).transpose()?.unwrap_or_default();
        let mut complete = Vec::new();
        for view in &training_views {
            if let Some(candidate) = Candidate::new(view.as_ref(), &training_observations, &source)?
                && model_paths.contains(&choices::activities(&candidate, &training_observations)?)
            {
                complete.push(candidate);
            }
        }
        for view in &withheld_views {
            let candidate = Candidate::new(view.as_ref(), &observed, &source)?;
            let sequence = candidate
                .as_ref()
                .map(|candidate| choices::activities(candidate, &observed))
                .transpose()?;
            let complete_sequence = sequence
                .as_ref()
                .is_some_and(|sequence| model_paths.contains(sequence));
            let (supporting_traces, next_expected_ms) = if let Some(candidate) = &candidate {
                prefixes::support(
                    candidate,
                    &observed,
                    &complete,
                    &training_observations,
                    route.and_then(|route| route.choices.first()),
                )?
            } else {
                (0, None)
            };
            let end_ms = candidate.as_ref().map(|candidate| candidate.end_ms);
            let supported = route.is_some_and(|route| route.peers_complete);
            let matched = supported && complete_sequence && supporting_traces > 0;
            let censored = supported
                && supporting_traces > 0
                && end_ms
                    .zip(next_expected_ms)
                    .is_some_and(|(end, next)| end < next);
            checks.push(json!({"trace":trace(view.as_ref()),"source":source,"training":false,"status":if matched {"matched"} else if censored {"censored"} else {"unresolved"},
            "scope_end_ms":end_ms,"next_expected_ms":next_expected_ms,"supporting_traces":supporting_traces,
            "reason":if matched {"complete observed sequence matches a training path"} else if censored {"observed prefix ended before the next occurrence on every matching training path"} else {"unseen occurrence order, missing scope, incomplete model or overdue occurrence"}}));
        }
    }
    Ok(
        json!({"method":"automatic-draft-validation-v1","status":"evaluated","selection":"greatest-report-v1",
        "split":split,"model":model.evidence(),"checks":checks,"not_checked":not_checked}),
    )
}

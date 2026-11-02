use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{Candidate, Draft, Progression, Pull, Route, Signal, choices};

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

// shortcut: independently check complete raid paths; add timing and censored-path validation with their own evidence.
pub(super) fn check(pulls: &[Pull]) -> Result<Value> {
    let reports: BTreeSet<_> = pulls.iter().map(|pull| &pull.report).collect();
    let not_checked = [
        "final KDL execution and timing",
        "censored-path conformance",
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
    let model = Draft::new(&training, &Progression::new(&training), false)?;
    let model_paths = model
        .routes
        .iter()
        .map(paths)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let observed = Progression::new(&withheld);
    let checks = withheld.iter().map(|pull| {
        let view = pull.learning_view();
        let candidate = Candidate::new(view.as_ref(), &observed, &None)?;
        let sequence = candidate.as_ref().map(|candidate| choices::activities(candidate, &observed)).transpose()?;
        let matched = model.supports_observed_routes() && sequence.as_ref().is_some_and(|sequence| model_paths.contains(sequence));
        Ok(json!({"trace":trace(pull),"training":false,"status":if matched {"matched"} else {"unresolved"},
            "reason":if matched {"complete observed sequence matches a training path"} else {"unseen occurrence order, missing scope, incomplete model or censored path"}}))
    }).collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"method":"automatic-draft-validation-v1","status":"evaluated","selection":"greatest-report-v1",
        "split":split,"model":model.evidence(),"checks":checks,"not_checked":not_checked}),
    )
}

use anyhow::Result;
use serde_json::{Value, json};

use super::{Analysis, Pull, Signal, occurrences};

fn trace(pull: &Pull) -> String {
    format!("{}/{}", pull.report, pull.fight)
}

// One deterministic fold bounds work; retaining all clears in the final fit preserves rare evidence.
pub(super) fn check(
    pulls: &[&Pull],
    include: &impl Fn(&Signal) -> bool,
    boss_only: bool,
) -> Result<Value> {
    let mut completed: Vec<_> = pulls.iter().copied().filter(|pull| pull.kill).collect();
    completed.sort_by(|left, right| (&left.report, left.fight).cmp(&(&right.report, right.fight)));
    let base = json!({"selection":"greatest-report-and-fight-v1",
        "final_fit":"all-completed-inputs","not_checked":["final KDL execution and timing","branch path conformance","all possible validation splits","statistical independence","ACT signal equivalence"]});
    if completed.len() < 3 {
        return Ok(
            json!({"method":"automatic-validation-v1","policy":base,"status":"insufficient-completed-pulls","completed":completed.len(),"required":3}),
        );
    }
    let Some(held_out) = completed.pop() else {
        return Ok(
            json!({"method":"automatic-validation-v1","policy":base,"status":"unresolved","reason":"missing validation input"}),
        );
    };
    let split = json!({"training":completed.iter().map(|pull|trace(pull)).collect::<Vec<_>>(),"held_out":trace(held_out)});
    let bosses: std::collections::BTreeSet<_> = completed
        .iter()
        .flat_map(|pull| pull.boss_names.iter())
        .collect();
    let include = |signal: &Signal| {
        include(signal)
            && (!boss_only
                || signal
                    .source
                    .as_ref()
                    .is_some_and(|source| bosses.contains(source)))
    };
    let model = Analysis::common(&completed, include)
        .and_then(|common| occurrences::expand(&completed, common, include, !boss_only));
    let model = match model {
        Ok(model) if !model.events.is_empty() => model,
        Ok(_) => {
            return Ok(
                json!({"method":"automatic-validation-v1","policy":base,"split":split,"status":"unresolved","reason":"no common events in validation model"}),
            );
        }
        Err(error) => {
            return Ok(
                json!({"method":"automatic-validation-v1","policy":base,"split":split,"status":"unresolved","reason":"validation model failed","error":error.to_string()}),
            );
        }
    };
    let checks = occurrences::holdout_checks(&completed, &[held_out], &model.events, &include)?;
    let events:Vec<_>=model.events.iter().map(|event|json!({"activity":event.signal,"samples":event.samples,"occurrence":event.occurrence})).collect();
    Ok(
        json!({"method":"automatic-validation-v1","policy":base,"split":split,"status":"evaluated","model_events":events,"checks":checks,
        "model_diagnostics":model.diagnostics}),
    )
}

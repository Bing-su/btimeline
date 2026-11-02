use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use kdl::KdlNode;
use serde_json::{Value, json};

use super::{Analysis, Identifier, Progression, Spec, label, prop, slug, stats};
use crate::generate::mining::{HelperAssociation, HelperOccurrence};

fn observation(progression: &Progression, row: &HelperOccurrence) -> Result<Value> {
    let mut nodes = BTreeSet::new();
    for member in &row.members {
        nodes.extend(
            progression
                .bind(
                    &member.signal,
                    &std::collections::BTreeMap::from([(row.trace.clone(), member.at_ms)]),
                )?
                .into_values(),
        );
    }
    Ok(
        json!({"trace":row.trace,"actor":row.actor,"instance":row.instance,"iteration":row.iteration,"cycle_start_ms":row.cycle_start_ms,"members":row.members,"observation_nodes":nodes,"helper":true}),
    )
}

// Preserve the boss forecast: the same name can describe different actions within one cycle.
pub(super) fn render(
    analysis: &Analysis,
    progression: &Progression,
    association: &HelperAssociation,
    period: i64,
    prefix: &str,
) -> Result<(KdlNode, Value)> {
    let spec = Spec {
        position: 0,
        signals: vec![association.activity.clone()],
        variants: vec![vec![association.activity.clone()]],
    };
    let name = label(analysis, &spec)?;
    let Identifier::Ability(ability) = association.activity.identifier else {
        anyhow::bail!("Helper display needs an ability identifier")
    };
    let tick = stats(association.observations.iter().flat_map(|row| {
        row.members
            .iter()
            .map(|member| (row.trace.clone(), member.at_ms - row.cycle_start_ms))
    }))?
    .0;
    ensure!(
        tick < period,
        "Helper effect falls outside the inferred cycle"
    );
    let event_id = format!("{prefix}-{}-{ability:x}", slug(&name));
    let mut event = KdlNode::new("event");
    event.push(event_id.clone());
    event.push(tick as f64 / 10.0);
    prop(&mut event, "name", name);
    let observations = association
        .observations
        .iter()
        .map(|row| observation(progression, row))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        event,
        json!({"event":event_id,"offset_ms":tick*100,"activities":[association.activity],"sync":false,"helper_association":association,"observations":observations}),
    ))
}

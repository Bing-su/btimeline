use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use kdl::KdlNode;

use super::{Analysis, Branch, Identifier, Mode, Rendered, anchor, prop, schedule, signal, slug};

pub(super) fn render(
    analysis: &Analysis,
    choices: &[Branch],
    zeros: &BTreeMap<String, i64>,
) -> Result<Rendered> {
    let mut result = Rendered::default();
    ensure!(
        choices.is_empty() || zeros.values().all(|at| *at == 0),
        "Local order branches require a combat-origin scope"
    );
    for choice in choices {
        let Identifier::Ability(join) = choice.join.identifier else {
            anyhow::bail!("Branch continuation requires an ability")
        };
        let id = format!("choice-{}-before-{join:x}", slug(&choice.source));
        let mut branch = KdlNode::new("branch");
        branch.push(id.clone());
        prop(&mut branch, "open", choice.open as f64 / 10.0);
        prop(&mut branch, "close", choice.close as f64 / 10.0);
        for path in &choice.options {
            let Identifier::Ability(trigger) = path.trigger.identifier else {
                anyhow::bail!("Branch choice requires an ability")
            };
            let option_id = format!(
                "{}-{trigger:x}",
                slug(
                    analysis
                        .ability_names
                        .get(&trigger)
                        .and_then(|names| names.first())
                        .context("Missing choice name")?
                )
            );
            let context = format!("before-{join:x}-via-{trigger:x}");
            let mut option = KdlNode::new("option");
            option.push(option_id.clone());
            option.ensure_children().nodes_mut().push(signal(
                "when",
                "Ability",
                &[("id", &format!("{trigger:X}")), ("source", &choice.source)],
            ));
            let anchors = path
                .events
                .iter()
                .map(|row| anchor(analysis, row))
                .collect::<Result<Vec<_>>>()?;
            let (events, mut evidence) =
                schedule(&anchors, &path.triggers, Mode::Raid, Some(&context))?;
            option.ensure_children().nodes_mut().extend(events);
            for row in &mut evidence {
                let fields = row.as_object_mut().context("Invalid branch evidence")?;
                fields.insert("branch".into(), id.clone().into());
                fields.insert("option".into(), option_id.clone().into());
                fields.insert(
                    "selection_samples".into(),
                    serde_json::to_value(&path.triggers)?,
                );
            }
            result.events.extend(evidence);
            branch.ensure_children().nodes_mut().push(option);
        }
        result.nodes.push(branch);
    }
    Ok(result)
}

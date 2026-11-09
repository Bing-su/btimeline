use super::*;

fn local_id(node: &KdlNode) -> Result<String> {
    let id = arg_string(node, 0)?;
    ensure!(!id.contains('/'), "Local ID cannot contain '/': {id}");
    ensure!(
        matches!(node.name().value(), "event" | "reaction" | "loop")
            || !matches!(id.as_str(), "shared" | "reaction" | "loop"),
        "Reserved local ID: {id}"
    );
    Ok(id)
}

fn qualify(node: &mut KdlNode, owner: &str) -> Result<String> {
    let id = format!("{owner}/{}", local_id(node)?);
    *node.get_mut(0).context("Missing ID")? = KdlValue::String(id.clone());
    Ok(id)
}

fn schedule(node: &mut KdlNode, owner: &str, allowed: &[&str]) -> Result<()> {
    let Some(document) = node.children_mut() else {
        return Ok(());
    };
    for child in document.nodes_mut() {
        let role = child.name().value().to_owned();
        ensure!(
            allowed.contains(&role.as_str()),
            "Unsupported schedule child: {role}"
        );
        match role.as_str() {
            "event" => {
                qualify(child, owner)?;
            }
            "section" => {
                local_id(child)?;
                schedule(child, owner, &["event", "sync", "branch", "loop"])?;
            }
            "reaction" => {
                let id = qualify(child, &format!("{owner}/reaction"))?;
                schedule(child, &id, &["on", "event", "sync", "section"])?;
                child.set_name("sequence");
            }
            "loop" => {
                let id = qualify(child, &format!("{owner}/loop"))?;
                schedule(child, &id, &["until", "event", "sync", "branch"])?;
            }
            "branch" => {
                ensure!(
                    child.get("at").is_none(),
                    "Use choice for progression selection"
                );
                let id = qualify(child, owner)?;
                branch_body(child, &id)?;
            }
            "choice" => choice(child, owner)?,
            _ => {}
        }
    }
    Ok(())
}

fn branch_body(node: &mut KdlNode, id: &str) -> Result<()> {
    let Some(options) = node.children_mut() else {
        return Ok(());
    };
    for option in options.nodes_mut() {
        match option.name().value() {
            "option" => {
                let option_id = local_id(option)?;
                schedule(
                    option,
                    &format!("{id}/{option_id}"),
                    &["when", "requires", "event", "sync", "loop", "section"],
                )?;
            }
            "merge" => schedule(option, &format!("{id}/merge"), &["when", "event", "sync"])?,
            _ => {}
        }
    }
    Ok(())
}

fn choice(node: &mut KdlNode, owner: &str) -> Result<()> {
    let id = qualify(node, owner)?;
    let mut selection_end = 0;
    let Some(document) = node.children_mut() else {
        return Ok(());
    };
    for option in document
        .nodes_mut()
        .iter_mut()
        .filter(|node| node.name().value() == "option")
    {
        let option_id = qualify(option, &id)?;
        schedule(option, &option_id, &["when", "event", "sync", "section"])?;
        for when in named(option, "when") {
            ensure!(when.get("at").is_none(), "choice when inherits at");
            selection_end = selection_end.max(property_time(when, "after")?.unwrap_or(25));
        }
    }
    for group in document.nodes_mut() {
        let role = group.name().value().to_owned();
        if role != "shared" {
            continue;
        }
        schedule(group, &format!("{id}/{role}"), &["event"])?;
        if role == "shared" {
            for event in children(group) {
                ensure!(
                    arg_time(event, 1)? >= selection_end,
                    "Shared event precedes selection close: {}",
                    arg_string(event, 0)?
                );
            }
        }
    }
    node.set_name("branch");
    Ok(())
}

fn references(node: &mut KdlNode, groups: &BTreeSet<String>) -> Result<()> {
    if let Some(value) = node.get("from-phase") {
        let group = string(value)?;
        ensure!(
            node.get("from").is_none() && named(node, "from").next().is_none(),
            "from-phase and from are mutually exclusive"
        );
        ensure!(groups.contains(&group), "Unknown from-phase: {group}");
        node.remove("from-phase");
        node.insert("from", group);
    } else {
        let mut refs = node
            .get("from")
            .map(string)
            .transpose()?
            .into_iter()
            .collect::<Vec<_>>();
        for list in named(node, "from") {
            refs.extend(
                args(list)
                    .into_iter()
                    .map(string)
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        ensure!(
            refs.iter().all(|id| id.contains('/')),
            "from requires a canonical progression reference; use from-phase for groups"
        );
    }
    if let Some(document) = node.children_mut() {
        for child in document.nodes_mut() {
            references(child, groups)?;
        }
    }
    Ok(())
}

pub(super) fn root(root: &mut KdlNode) -> Result<()> {
    ensure!(
        root.get(0).and_then(KdlValue::as_integer) == Some(2),
        "Expected timeline version 2"
    );
    let mut groups = BTreeSet::new();
    let Some(document) = root.children_mut() else {
        return Ok(());
    };
    for phase in document.nodes_mut() {
        if phase.name().value() == "event" {
            local_id(phase)?;
        }
        if phase.name().value() != "phase" {
            continue;
        }
        let id = local_id(phase)?;
        ensure!(groups.insert(id.clone()), "Duplicate phase: {id}");
        ensure!(
            !children(phase).is_empty()
                && children(phase)
                    .iter()
                    .all(|node| node.name().value() == "step"),
            "phase contains only steps"
        );
        let Some(steps) = phase.children_mut() else {
            continue;
        };
        for step in steps.nodes_mut() {
            let step_id = qualify(step, &id)?;
            ensure!(
                named(step, "choice").count() <= 1,
                "step permits at most one choice"
            );
            schedule(
                step,
                &step_id,
                &[
                    "enter", "after", "event", "sync", "section", "reaction", "loop", "branch",
                    "choice",
                ],
            )?;
            step.set_name("sequence");
        }
    }
    references(root, &groups)
}

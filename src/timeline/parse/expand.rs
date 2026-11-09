use super::*;
mod normalize;

#[derive(Clone)]
struct Source {
    id: String,
    offset: Tick,
}

type Aliases = BTreeMap<String, Vec<Source>>;

fn body(node: &mut KdlNode, nodes: Vec<KdlNode>) {
    let mut document = kdl::KdlDocument::new();
    *document.nodes_mut() = nodes;
    node.set_children(document);
}

fn phase(id: &str, nodes: Vec<KdlNode>) -> KdlNode {
    let mut node = KdlNode::new("phase");
    node.push(id);
    body(&mut node, nodes);
    node
}

fn named<'a>(node: &'a KdlNode, name: &'a str) -> impl Iterator<Item = &'a KdlNode> {
    children(node)
        .iter()
        .filter(move |child| child.name().value() == name)
}

fn add_joins(
    node: &KdlNode,
    sources: &[Source],
    joins: &mut BTreeMap<String, Vec<KdlNode>>,
) -> Result<()> {
    for join in named(node, "join") {
        shape(join, 1, &["after", "recover-nominal"])?;
        let recover_nominal = join
            .get("recover-nominal")
            .map(|value| value.as_bool().context("recover-nominal must be boolean"))
            .transpose()?
            .unwrap_or(false);
        let target = arg_string(join, 0)?;
        let after = property_time(join, "after")?.context("join requires after")?;
        ensure!(after > 0, "join after must be positive");
        ensure!(
            children(join)
                .iter()
                .all(|child| child.name().value() == "when"),
            "join permits only when"
        );
        let controls = joins.entry(target).or_default();
        for (index, source) in sources.iter().enumerate() {
            let mut deadline = KdlNode::new("after");
            deadline.push((after + source.offset) as f64 / 10.0);
            deadline.insert("from", source.id.as_str());
            controls.push(deadline);
            if index == 0 && !recover_nominal {
                continue;
            }
            for when in children(join) {
                ensure!(
                    ["from", "at", "resume-at"]
                        .iter()
                        .all(|key| when.get(*key).is_none()),
                    "join when inherits from and at"
                );
                let mut enter = when.clone();
                enter.set_name("enter");
                enter.insert("from", source.id.as_str());
                enter.insert("at", (after + source.offset) as f64 / 10.0);
                controls.push(enter);
            }
        }
    }
    Ok(())
}

fn shared_events(branch: &KdlNode) -> Result<Vec<KdlNode>> {
    let groups: Vec<_> = named(branch, "shared").collect();
    ensure!(groups.len() <= 1, "Branch permits one shared body");
    let mut events = Vec::new();
    for group in groups {
        shape(group, 0, &[])?;
        ensure!(!children(group).is_empty(), "shared requires events");
        for event in children(group) {
            ensure!(
                event.name().value() == "event",
                "shared permits only events"
            );
            events.push(event.clone());
        }
    }
    Ok(events)
}

fn branch(
    node: &KdlNode,
    from: &str,
    aliases: &mut Aliases,
    joins: &mut BTreeMap<String, Vec<KdlNode>>,
) -> Result<(Vec<KdlNode>, Vec<KdlNode>)> {
    shape(node, 1, &["at"])?;
    let id = arg_string(node, 0)?;
    let at = property_time(node, "at")?.context("Flow branch requires at")?;
    let options: Vec<_> = named(node, "option").collect();
    ensure!(options.len() >= 2, "Choice needs at least two options");
    ensure!(
        children(node)
            .iter()
            .all(|part| matches!(part.name().value(), "option" | "join" | "shared")),
        "Unsupported flow branch child"
    );
    let mut sources = vec![Source {
        id: from.into(),
        offset: at,
    }];
    let mut phases = Vec::new();
    for option in options {
        shape(option, 1, &[])?;
        let option_id = arg_string(option, 0)?;
        let whens: Vec<_> = named(option, "when").collect();
        ensure!(!whens.is_empty(), "Flow option requires when");
        let mut nodes = Vec::new();
        for when in whens {
            ensure!(
                when.get("from").is_none() && when.get("resume-at").is_none(),
                "Flow option when cannot specify from or resume-at"
            );
            let mut copy = when.clone();
            copy.set_name("enter");
            copy.insert("from", from);
            if copy.get("at").is_none() {
                copy.insert("at", at as f64 / 10.0);
            }
            nodes.push(copy);
        }
        nodes.extend(shared_events(node)?);
        nodes.extend(
            children(option)
                .iter()
                .filter(|part| part.name().value() != "when")
                .cloned(),
        );
        sources.push(Source {
            id: option_id.clone(),
            offset: 0,
        });
        phases.push(phase(&option_id, nodes));
    }
    ensure!(!aliases.contains_key(&id), "Duplicate flow reference: {id}");
    add_joins(node, &sources, joins)?;
    aliases.insert(id, sources);
    let mut predictions = Vec::new();
    for mut event in shared_events(node)? {
        body(&mut event, Vec::new());
        shift_scheduled(&mut event, at)?;
        predictions.push(event);
    }
    Ok((phases, predictions))
}

fn flow_phase(
    node: &KdlNode,
    aliases: &mut Aliases,
    joins: &mut BTreeMap<String, Vec<KdlNode>>,
) -> Result<Vec<KdlNode>> {
    shape(node, 1, &[])?;
    let id = arg_string(node, 0)?;
    ensure!(
        children(node)
            .iter()
            .all(|part| part.name().value() == "sequence"),
        "A flow phase contains only flow sequences"
    );
    let mut phases = Vec::new();
    for sequence in children(node) {
        shape(sequence, 1, &[])?;
        let sequence_id = arg_string(sequence, 0)?;
        let mut nodes = Vec::new();
        let phase_index = phases.len();
        phases.push(phase(&sequence_id, Vec::new()));
        for part in children(sequence) {
            if part.name().value() == "branch" && part.get("at").is_some() {
                let (options, predictions) = branch(part, &sequence_id, aliases, joins)?;
                phases.extend(options);
                nodes.extend(predictions);
            } else {
                nodes.push(part.clone());
            }
        }
        body(
            phases
                .get_mut(phase_index)
                .context("Missing flow sequence")?,
            nodes,
        );
    }
    ensure!(!aliases.contains_key(&id), "Duplicate flow reference: {id}");
    aliases.insert(
        id,
        phases
            .iter()
            .map(|node| {
                Ok(Source {
                    id: arg_string(node, 0)?,
                    offset: 0,
                })
            })
            .collect::<Result<_>>()?,
    );
    Ok(phases)
}

fn resolve(mut node: KdlNode, aliases: &Aliases) -> Result<Vec<KdlNode>> {
    let role = node.name().value().to_owned();
    if !matches!(role.as_str(), "enter" | "after" | "boss-complete") {
        if node.children().is_some() {
            let nodes = children(&node)
                .iter()
                .cloned()
                .map(|part| resolve(part, aliases))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .collect();
            body(&mut node, nodes);
        }
        return Ok(vec![node]);
    }
    let lists: Vec<_> = named(&node, "from").collect();
    ensure!(
        lists.len() <= 1 && !(lists.len() == 1 && node.get("from").is_some()),
        "Specify one from property or list"
    );
    let names = if let Some(list) = lists.first() {
        ensure!(
            role == "enter" && children(list).is_empty(),
            "Only enter supports a from list"
        );
        shape(list, args(list).len(), &[])?;
        let names = args(list)
            .into_iter()
            .map(string)
            .collect::<Result<Vec<_>>>()?;
        ensure!(!names.is_empty(), "Empty from list");
        names
    } else {
        node.get("from")
            .map(string)
            .transpose()?
            .into_iter()
            .collect()
    };
    ensure!(
        names.iter().collect::<BTreeSet<_>>().len() == names.len(),
        "Duplicate phase in from list"
    );
    if names.is_empty() {
        return Ok(vec![node]);
    }
    let mut result = Vec::new();
    for name in names {
        ensure!(
            !name.contains('/') || !aliases.contains_key(&name),
            "from must name a step or option, not a choice"
        );
        let fallback = vec![Source {
            id: name.clone(),
            offset: 0,
        }];
        for source in aliases.get(&name).unwrap_or(&fallback) {
            let mut copy = node.clone();
            copy.insert("from", source.id.as_str());
            if !lists.is_empty() {
                let nodes = children(&copy)
                    .iter()
                    .filter(|part| part.name().value() != "from")
                    .cloned()
                    .collect();
                body(&mut copy, nodes);
            }
            shift_source(&mut copy, &role, source.offset)?;
            result.push(copy);
        }
    }
    Ok(result)
}

fn shift_source(node: &mut KdlNode, role: &str, offset: Tick) -> Result<()> {
    if offset == 0 {
        return Ok(());
    }
    if role == "after" {
        shift_argument(node, 0, offset)
    } else {
        shift_property(node, "at", offset)
    }
}

fn is_flow(node: &KdlNode) -> bool {
    node.name().value() == "phase"
}

pub(super) fn event_syncs(node: &mut KdlNode) -> Result<bool> {
    let mut changed = false;
    ensure!(
        node.name().value() != "sequence" || node.get("at").is_none(),
        "Use section for scheduled groups"
    );
    if node.name().value() == "section" {
        shape(node, 1, &["at"])?;
        property_time(node, "at")?.context("section requires at")?;
        ensure!(!children(node).is_empty(), "section requires a body");
        changed = true;
    }
    let event_at = if node.name().value() == "event" {
        Some(arg_time(node, 1)?)
    } else {
        None
    };
    let Some(document) = node.children_mut() else {
        return Ok(changed);
    };
    for child in document.nodes_mut() {
        if let Some(at) = event_at
            && child.name().value() == "sync"
            && args(child).len() == 1
        {
            child
                .entries_mut()
                .insert(0, kdl::KdlEntry::new(at as f64 / 10.0));
            changed = true;
        }
        changed |= event_syncs(child)?;
    }
    let mut flattened = Vec::new();
    for child in document.nodes() {
        if child.name().value() == "section" {
            let offset = property_time(child, "at")?.context("section requires at")?;
            for part in children(child) {
                let mut copy = part.clone();
                shift_scheduled(&mut copy, offset)?;
                flattened.push(copy);
            }
        } else {
            flattened.push(child.clone());
        }
    }
    *document.nodes_mut() = flattened;
    Ok(changed)
}

pub(super) fn document(
    source: &str,
    declarations: &mut BTreeMap<usize, usize>,
    choices: &mut Vec<super::super::Choice>,
) -> Result<kdl::KdlDocument> {
    let mut document: kdl::KdlDocument = source
        .parse()
        .map_err(|error| anyhow::anyhow!("Invalid KDL: {error:?}"))?;
    fn visit(node: &KdlNode, declarations: &mut BTreeMap<usize, usize>) {
        declarations.insert(node.span().offset(), declarations.len());
        for child in children(node) {
            visit(child, declarations);
        }
    }
    for node in document.nodes() {
        visit(node, declarations);
    }
    let [root] = document.nodes_mut().as_mut_slice() else {
        return Ok(document);
    };
    if root.name().value() != "timeline" {
        return Ok(document);
    }
    normalize::root(root)?;
    let implicit_syncs = event_syncs(root)?;
    for phase in named(root, "phase") {
        for step in children(phase) {
            for choice in named(step, "branch").filter(|node| node.get("at").is_some()) {
                choices.push(super::super::Choice {
                    id: arg_string(choice, 0)?,
                    step_id: arg_string(step, 0)?,
                    at: property_time(choice, "at")?.context("Choice requires at")?,
                    option_ids: named(choice, "option")
                        .map(|option| arg_string(option, 0))
                        .collect::<Result<_>>()?,
                });
            }
        }
    }
    if !children(root).iter().any(is_flow) && !implicit_syncs {
        return Ok(document);
    }
    let mut aliases = Aliases::new();
    let mut joins = BTreeMap::new();
    let mut nodes = Vec::new();
    for node in children(root) {
        if is_flow(node) {
            nodes.extend(flow_phase(node, &mut aliases, &mut joins)?);
        } else {
            nodes.push(node.clone());
        }
    }
    let phase_ids = nodes
        .iter()
        .filter(|node| node.name().value() == "phase")
        .map(|node| arg_string(node, 0))
        .collect::<Result<BTreeSet<_>>>()?;
    for (target, mut controls) in joins {
        let phase = nodes
            .iter_mut()
            .find(|node| {
                node.name().value() == "phase" && arg_string(node, 0).is_ok_and(|id| id == target)
            })
            .with_context(|| format!("Unknown join target: {target}"))?;
        controls.extend(children(phase).iter().cloned());
        body(phase, controls);
    }
    ensure!(
        aliases.keys().all(|alias| !phase_ids.contains(alias)),
        "Flow reference conflicts with phase"
    );
    let nodes = nodes
        .into_iter()
        .map(|node| resolve(node, &aliases))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    body(root, nodes);
    Ok(document)
}

use std::collections::{BTreeMap, BTreeSet};

mod expand;

use anyhow::{Context, Result, bail, ensure};
use kdl::{KdlNode, KdlValue};
use regress::Regex;

use super::{
    Branch,
    BranchOption,
    Event,
    Field,
    Loop,
    MAX_MS,
    Merge,
    Phase,
    Signal,
    Tick,
    Timeline,
};

fn children(node: &KdlNode) -> &[KdlNode] {
    node.children().map_or(&[], |doc| doc.nodes())
}

fn check_schedule_syncs(
    syncs: &[(usize, Signal)],
    latest: Tick,
    every: Option<Tick>,
) -> Result<()> {
    if let Some(every) = every {
        for (_, signal) in syncs {
            ensure!(
                signal.at.is_some_and(|at| latest + at < every),
                "Sync crosses the iteration boundary"
            );
        }
    }
    Ok(())
}

fn args(node: &KdlNode) -> Vec<&KdlValue> {
    node.entries()
        .iter()
        .filter(|e| e.name().is_none())
        .map(|e| e.value())
        .collect()
}

fn string(value: &KdlValue) -> Result<String> {
    let value = value.as_string().context("Expected a string")?;
    ensure!(!value.is_empty(), "Expected a nonempty string");
    Ok(value.to_owned())
}

fn ticks(value: &KdlValue) -> Result<Tick> {
    let number = match value {
        KdlValue::Integer(n) => *n as f64,
        KdlValue::Float(n) => *n,
        _ => bail!("Expected seconds, e.g. 42.5"),
    };
    let scaled = number * 10.0;
    ensure!(
        number.is_finite() && number >= 0.0 && number <= (MAX_MS / 1000) as f64,
        "Time must be finite, nonnegative and within {MAX_MS} milliseconds"
    );
    ensure!(
        (scaled - scaled.round()).abs() <= 1e-7,
        "Time must use 0.1-second units, e.g. 42.5"
    );
    Ok(scaled.round() as Tick)
}

fn property_time(node: &KdlNode, key: &str) -> Result<Option<Tick>> {
    node.get(key).map(ticks).transpose()
}

fn shape(node: &KdlNode, count: usize, properties: &[&str]) -> Result<()> {
    ensure!(node.ty().is_none(), "Type annotations are unsupported");
    ensure!(
        args(node).len() == count,
        "{} requires {count} positional arguments",
        node.name().value()
    );
    let mut seen = BTreeSet::new();
    for entry in node.entries() {
        ensure!(entry.ty().is_none(), "Type annotations are unsupported");
        if let Some(name) = entry.name() {
            ensure!(
                seen.insert(name.value()),
                "Duplicate property {}",
                name.value()
            );
            ensure!(
                properties.contains(&name.value()),
                "Unknown property {}; remove or correct it",
                name.value()
            );
        }
    }
    Ok(())
}

fn arg_string(node: &KdlNode, index: usize) -> Result<String> {
    string(args(node).get(index).context("Missing string argument")?)
}

fn arg_time(node: &KdlNode, index: usize) -> Result<Tick> {
    ticks(args(node).get(index).context("Missing time argument")?)
}

fn shift_property(node: &mut KdlNode, key: &str, offset: Tick) -> Result<()> {
    if let Some(value) = node.get_mut(key) {
        *value = KdlValue::Float((ticks(value)? + offset) as f64 / 10.0);
    }
    Ok(())
}

fn shift_argument(node: &mut KdlNode, index: usize, offset: Tick) -> Result<()> {
    let value = node.get_mut(index).context("Missing scheduled time")?;
    *value = KdlValue::Float((ticks(value)? + offset) as f64 / 10.0);
    Ok(())
}

fn shift_scheduled(node: &mut KdlNode, offset: Tick) -> Result<()> {
    match node.name().value() {
        "sync" => shift_argument(node, 0, offset)?,
        "event" => {
            shift_argument(node, 1, offset)?;
            if let Some(body) = node.children_mut() {
                for sync in body.nodes_mut() {
                    shift_argument(sync, 0, offset)?;
                }
            }
        }
        "branch" => {
            shift_property(node, "open", offset)?;
            shift_property(node, "close", offset)?;
            if let Some(body) = node.children_mut() {
                for choice in body.nodes_mut() {
                    shift_choice_windows(choice, offset)?;
                }
            }
        }
        "loop" => {
            shift_property(node, "start", offset)?;
            if let Some(body) = node.children_mut() {
                for child in body
                    .nodes_mut()
                    .iter_mut()
                    .filter(|child| child.name().value() == "until")
                {
                    shift_property(child, "at", offset)?;
                }
            }
        }
        name => bail!("Unsupported scheduled sequence child: {name}"),
    }
    Ok(())
}

fn shift_choice_windows(choice: &mut KdlNode, offset: Tick) -> Result<()> {
    if let Some(body) = choice.children_mut() {
        for child in body
            .nodes_mut()
            .iter_mut()
            .filter(|child| child.name().value() == "when")
        {
            shift_property(child, "at", offset)?;
        }
    }
    Ok(())
}

fn scheduled_phase(node: &KdlNode) -> Result<KdlNode> {
    let mut result = node.clone();
    let mut body = Vec::new();
    let mut sequences = BTreeSet::new();
    for child in children(node) {
        if child.name().value() != "sequence" {
            body.push(child.clone());
            continue;
        }
        let sequence_id = arg_string(child, 0)?;
        ensure!(
            sequences.insert(sequence_id.clone()),
            "Duplicate sequence ID {sequence_id} in phase"
        );
        let Some(at) = property_time(child, "at")? else {
            body.push(child.clone());
            continue;
        };
        shape(child, 1, &["at"])?;
        ensure!(
            !children(child).is_empty(),
            "Scheduled sequence requires a body"
        );
        for part in children(child) {
            let mut part = part.clone();
            shift_scheduled(&mut part, at)?;
            body.push(part);
        }
    }
    if let Some(document) = result.children_mut() {
        *document.nodes_mut() = body;
    }
    Ok(result)
}

fn location(source: &str, node: &KdlNode) -> String {
    let prefix = source.get(..node.span().offset()).unwrap_or("");
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let col = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{} at {line}:{col}", node.name().value())
}

// Prove overlap only for literal conditions; regex intersections remain unknown.
fn literal_signals_overlap(left: &Signal, right: &Signal) -> bool {
    if left.kind != right.kind {
        return false;
    }
    if let Some((a, b)) = left.at.zip(right.at)
        && (left
            .after
            .is_some_and(|after| a + after <= b - right.before)
            || right
                .after
                .is_some_and(|after| b + after <= a - left.before))
    {
        return false;
    }
    let literals = |s: &Signal| s.fields.values().all(|v| matches!(v, Field::Literal(_)));
    if !literals(left) || !literals(right) {
        return false;
    }
    left.fields.iter().all(|(name, a)| {
        let Some(b) = right.fields.get(name) else {
            return true;
        };
        match (a, b) {
            (Field::Literal(a), Field::Literal(b)) => a.iter().any(|value| b.contains(value)),
            _ => false,
        }
    })
}

// Identical prerequisites enable both options together whenever those prerequisites hold.
fn validate_option_overlaps(node: &KdlNode, open: Tick, close: Tick) -> Result<()> {
    let options: Vec<_> = children(node)
        .iter()
        .filter(|child| child.name().value() == "option")
        .collect();
    let candidates = options
        .iter()
        .map(|option| {
            let whens = children(option)
                .iter()
                .filter(|part| part.name().value() == "when")
                .map(|part| signal(part, "when"))
                .collect::<Result<Vec<_>>>()?;
            let requires = children(option)
                .iter()
                .filter(|part| part.name().value() == "requires")
                .map(|part| Ok((arg_string(part, 0)?, arg_string(part, 1)?)))
                .collect::<Result<BTreeSet<_>>>()?;
            Ok((arg_string(option, 0)?, whens, requires))
        })
        .collect::<Result<Vec<_>>>()?;
    // Only a conflict inside the branch window can select two entry points.
    let active = |signal: &&Signal| {
        signal.at.is_none_or(|at| {
            at - signal.before < close && signal.after.is_none_or(|after| at + after > open)
        })
    };
    for (index, (left_id, left, left_requires)) in candidates.iter().enumerate() {
        for (index, a) in left.iter().enumerate().filter(|(_, signal)| active(signal)) {
            ensure!(
                !left
                    .iter()
                    .skip(index + 1)
                    .filter(active)
                    .any(|b| { a.resume_at != b.resume_at && literal_signals_overlap(a, b) }),
                "Conflicting entry offsets for option {left_id}: distinguish their signals"
            );
        }
        for (right_id, right, right_requires) in candidates.iter().skip(index + 1) {
            ensure!(
                left_requires != right_requires
                    || !left
                        .iter()
                        .any(|a| right.iter().any(|b| literal_signals_overlap(a, b))),
                "Conflicting options {left_id} and {right_id} with identical prerequisites: distinguish their signals"
            );
        }
    }
    Ok(())
}

// Keep literal and regex field validation identical across signal roles.
fn signal_fields(node: &KdlNode, reserved: &[&str]) -> Result<BTreeMap<String, Field>> {
    let mut fields = BTreeMap::new();
    for entry in node.entries() {
        if let Some(name) = entry.name()
            && !reserved.contains(&name.value())
        {
            fields.insert(
                name.value().to_owned(),
                Field::Literal(vec![
                    entry
                        .value()
                        .as_string()
                        .context("Log field must be a string")?
                        .to_owned(),
                ]),
            );
        }
    }
    for child in children(node) {
        if node.name().value() == "enter" && child.name().value() == "from" {
            continue;
        }
        ensure!(
            matches!(child.name().value(), "field" | "regex"),
            "Unknown signal child {}",
            child.name().value()
        );
        ensure!(
            children(child).is_empty(),
            "Signal field cannot have children"
        );
        ensure!(
            args(child).len() >= 2,
            "field/regex needs a name and at least one value"
        );
        shape(child, args(child).len(), &[])?;
        let key = arg_string(child, 0)?;
        ensure!(!fields.contains_key(&key), "Duplicate signal field {key}");
        let values = args(child)
            .into_iter()
            .skip(1)
            .map(|v| {
                Ok(v.as_string()
                    .context("Field condition must be a string")?
                    .to_owned())
            })
            .collect::<Result<Vec<_>>>()?;
        let condition = if child.name().value() == "regex" {
            Field::Regex(
                values
                    .iter()
                    // Anchor without capturing groups or `$`'s final-newline exception.
                    .map(|p| {
                        // Validate first: wrapping an unfinished class like `[` could hide its error.
                        Regex::new(p)?;
                        Regex::new(&format!("^(?:{p})(?![\\s\\S])"))
                    })
                    .collect::<std::result::Result<_, _>>()?,
            )
        } else {
            Field::Literal(values)
        };
        fields.insert(key, condition);
    }
    Ok(fields)
}

fn signal(node: &KdlNode, role: &str) -> Result<Signal> {
    signal_from(node, role, None)
}

fn signal_from(node: &KdlNode, role: &str, source: Option<String>) -> Result<Signal> {
    if role == "on" {
        ensure!(
            ["at", "before", "after", "from", "resume-at"]
                .iter()
                .all(|key| node.get(*key).is_none()),
            "sequence on does not support windows, from or resume-at"
        );
    }
    let sync = role == "sync";
    let timed = matches!(role, "sync" | "enter" | "when" | "merge-when" | "until");
    let reserved: &[&str] = match role {
        "sync" => &["before", "after"],
        "enter" => &["from", "at", "before", "after", "resume-at"],
        "boss-complete" => &["from"],
        "when" => &["at", "before", "after", "resume-at"],
        "merge-when" | "until" => &["at", "before", "after"],
        _ => &[],
    };
    let names: Vec<_> = node
        .entries()
        .iter()
        .filter_map(|e| e.name().map(|n| n.value()))
        .collect();
    shape(node, if sync { 2 } else { 1 }, &names)?;
    let fields = signal_fields(node, reserved)?;
    let at = if sync {
        Some(arg_time(node, 0)?)
    } else if timed {
        property_time(node, "at")?
    } else {
        None
    };
    let before = if timed {
        property_time(node, "before")?
    } else {
        None
    };
    let after = if timed {
        property_time(node, "after")?
    } else {
        None
    };
    ensure!(
        after
            .is_none_or(|width| before.unwrap_or(if role == "until" { 0 } else { 25 }) + width > 0),
        "Window must have positive width"
    );
    if role == "until" {
        ensure!(at.is_some(), "until requires at");
    } else {
        ensure!(
            before.is_some() == after.is_some(),
            "Specify both before and after, or omit both"
        );
        ensure!(at.is_some() || before.is_none(), "Window requires at");
    }
    let from = if matches!(role, "enter" | "boss-complete") {
        source.or(node.get("from").map(string).transpose()?)
    } else {
        None
    };
    if role == "enter" && at.is_some() {
        ensure!(from.is_some(), "enter with at requires from");
    }
    let resume_at = if matches!(role, "enter" | "when") {
        property_time(node, "resume-at")?
    } else {
        None
    };
    ensure!(
        resume_at.is_none() || from.is_none(),
        "resume-at requires an entry without from"
    );
    Ok(Signal {
        kind: arg_string(node, usize::from(sync))?,
        fields,
        at,
        before: before.unwrap_or(if role == "until" { 0 } else { 25 }),
        after: if role == "until" {
            after
        } else {
            Some(after.unwrap_or(25))
        },
        from,
        resume_at,
    })
}

fn enter_signals(node: &KdlNode) -> Result<Vec<Signal>> {
    let lists: Vec<_> = children(node)
        .iter()
        .filter(|child| child.name().value() == "from")
        .collect();
    ensure!(lists.len() <= 1, "enter permits one from list");
    let Some(list) = lists.first() else {
        return Ok(vec![signal(node, "enter")?]);
    };
    ensure!(
        node.get("from").is_none(),
        "Specify from as a property or a list, not both"
    );
    ensure!(!args(list).is_empty(), "from requires at least one phase");
    shape(list, args(list).len(), &[])?;
    ensure!(children(list).is_empty(), "from cannot have children");
    let mut seen = BTreeSet::new();
    args(list)
        .into_iter()
        .map(|value| {
            let from = string(value)?;
            ensure!(seen.insert(from.clone()), "Duplicate phase in from list");
            signal_from(node, "enter", Some(from))
        })
        .collect()
}

#[derive(Default)]
struct Validation {
    shared_sources: BTreeMap<String, usize>,
    ids: BTreeMap<String, BTreeSet<String>>,
    options: BTreeMap<String, BTreeSet<String>>,
    requires: Vec<(String, String, String)>,
    phase_refs: Vec<String>,
    branches: BTreeMap<String, Branch>,
    loops: BTreeMap<String, Loop>,
    event_count: usize,
}

impl Validation {
    fn id(&mut self, node: &KdlNode) -> Result<String> {
        let id = arg_string(node, 0)?;
        ensure!(
            self.ids
                .entry(node.name().value().to_owned())
                .or_default()
                .insert(id.clone()),
            "Duplicate {} ID {id}",
            node.name().value()
        );
        Ok(id)
    }

    fn event(&mut self, node: &KdlNode, latest: Tick, every: Option<Tick>) -> Result<Event> {
        shape(node, 2, &["name", "duration", "visible"])?;
        let candidate = arg_string(node, 0)?;
        let shared = candidate.split('/').any(|part| part == "shared");
        let id = if shared && self.shared_sources.get(&candidate) == Some(&node.span().offset()) {
            candidate
        } else {
            let id = self.id(node)?;
            if shared {
                self.shared_sources.insert(id.clone(), node.span().offset());
            }
            id
        };
        let at = arg_time(node, 1)?;
        if let Some(every) = every {
            ensure!(
                latest + at < every,
                "Event {id} can cross the iteration boundary; shorten the window or move it earlier"
            );
        }
        let name = string(node.get("name").context("event requires name")?)?;
        let duration = property_time(node, "duration")?.unwrap_or(0);
        let visible = node
            .get("visible")
            .map(|v| v.as_bool().context("visible must be a boolean"))
            .transpose()?
            .unwrap_or(true);
        let mut syncs = Vec::new();
        for child in children(node) {
            ensure!(
                child.name().value() == "sync",
                "Only sync is allowed inside event"
            );
            let s = signal(child, "sync")?;
            if let (Some(every), Some(at)) = (every, s.at) {
                ensure!(
                    latest + at < every,
                    "Sync for {id} can cross the iteration boundary"
                );
            }
            syncs.push(s);
        }
        self.event_count += 1;
        Ok(Event {
            order: self.event_count,
            source_start: node.span().offset(),
            source_end: node.span().offset() + node.span().len(),
            id,
            at,
            name,
            duration,
            visible,
            syncs,
        })
    }

    fn body(
        &mut self,
        nodes: &[KdlNode],
        parent: &str,
        in_loop: bool,
        latest: Tick,
        every: Option<Tick>,
        source: &str,
    ) -> Result<Vec<Event>> {
        let mut events = Vec::new();
        for node in nodes {
            if let Some(event) = self
                .body_node(node, parent, in_loop, latest, every, source)
                .with_context(|| location(source, node))?
            {
                events.push(event);
            }
        }
        Ok(events)
    }

    // Validate a child while its caller retains source diagnostics.
    fn body_node(
        &mut self,
        node: &KdlNode,
        parent: &str,
        in_loop: bool,
        latest: Tick,
        every: Option<Tick>,
        source: &str,
    ) -> Result<Option<Event>> {
        match node.name().value() {
            "event" => return Ok(Some(self.event(node, latest, every)?)),
            "branch" if matches!(parent, "phase" | "loop") => {
                self.branch(node, in_loop, every, source)?
            }
            "loop" if matches!(parent, "phase" | "option") => {
                ensure!(
                    !in_loop,
                    "Nested loops, including loops inside options of a loop, are forbidden"
                );
                self.loop_node(node, source)?;
            }
            name => bail!("Unsupported child {name} in {parent}"),
        }
        Ok(None)
    }

    // Parse a selected path without changing branch-level uniqueness checks.
    fn option_node(
        &mut self,
        child: &KdlNode,
        branch: &str,
        close: Tick,
        every: Option<Tick>,
        source: &str,
        options: &mut BTreeSet<String>,
    ) -> Result<BranchOption> {
        shape(child, 1, &["fallback"])?;
        let fallback = child
            .get("fallback")
            .map(|value| value.as_bool().context("fallback must be a boolean"))
            .transpose()?
            .unwrap_or(false);
        let option = arg_string(child, 0)?;
        ensure!(
            options.insert(option.clone()),
            "Duplicate option in {branch}"
        );

        let mut body = Vec::new();
        let mut whens = Vec::new();
        let mut requires = Vec::new();
        let mut syncs = Vec::new();
        for part in children(child) {
            match part.name().value() {
                "sync" => syncs.push((part.span().offset(), signal(part, "sync")?)),
                "when" => {
                    whens.push(signal(part, "when")?);
                }
                "requires" => {
                    shape(part, 2, &[])?;
                    ensure!(children(part).is_empty(), "requires has no children");
                    let target = arg_string(part, 0)?;
                    let choice = arg_string(part, 1)?;
                    ensure!(
                        requires
                            .iter()
                            .all(
                                |(required_branch, required_choice)| required_branch != &target
                                    || required_choice == &choice
                            ),
                        "Option {branch}/{option} requires conflicting choices for {target}"
                    );
                    self.requires
                        .push((branch.to_owned(), target.clone(), choice.clone()));
                    requires.push((target, choice));
                }
                _ => body.push(part.clone()),
            }
        }
        ensure!(!whens.is_empty(), "option requires at least one when");
        let events = self.body(&body, "option", every.is_some(), close - 1, every, source)?;
        check_schedule_syncs(&syncs, close - 1, every)?;
        let loops = self.take_loops(&body)?;
        ensure!(
            loops.is_empty() || whens.iter().all(|signal| signal.resume_at.is_none()),
            "option resume-at requires a body without loops"
        );
        Ok(BranchOption {
            syncs,
            id: option,
            fallback,
            whens,
            requires,
            events,
            loops,
        })
    }

    // Bound common-path events using every alternative merge window.
    fn merge_node(
        &mut self,
        child: &KdlNode,
        in_loop: bool,
        every: Option<Tick>,
        source: &str,
    ) -> Result<Merge> {
        shape(child, 0, &[])?;

        let mut body = Vec::new();
        let mut whens = Vec::new();
        let mut latest_merge = 0;
        let mut syncs = Vec::new();
        for part in children(child) {
            if part.name().value() == "sync" {
                syncs.push((part.span().offset(), signal(part, "sync")?));
                continue;
            }
            if part.name().value() != "when" {
                body.push(part.clone());
                continue;
            }
            let s = signal(part, "merge-when")?;
            if in_loop {
                ensure!(
                    s.at.is_some() && part.get("before").is_some() && part.get("after").is_some(),
                    "merge inside loop requires explicit at, before and after"
                );
                latest_merge = latest_merge.max(s.at.unwrap_or(0) + s.after.unwrap_or(0));
            }
            whens.push(s);
        }
        ensure!(!whens.is_empty(), "merge requires when");
        let events = self.body(&body, "merge", in_loop, latest_merge, every, source)?;
        check_schedule_syncs(&syncs, latest_merge, every)?;
        Ok(Merge {
            whens,
            events,
            syncs,
        })
    }

    // Keep phase-local signals and their source diagnostics together.
    fn phase_node(&mut self, node: &KdlNode, source: &str) -> Result<Phase> {
        let expanded = scheduled_phase(node)?;
        let node = &expanded;
        shape(node, 1, &[])?;
        let id = self.id(node)?;
        let mut enters = Vec::new();
        let mut sequences = Vec::new();
        let mut sequence_ids = BTreeSet::new();
        let mut body = Vec::new();
        let mut after = Vec::new();
        let mut syncs = Vec::new();
        for child in children(node) {
            if child.name().value() == "enter" {
                let signals = enter_signals(child).with_context(|| location(source, child))?;
                self.phase_refs.extend(
                    signals
                        .iter()
                        .flat_map(|signal| signal.from.iter().cloned()),
                );
                enters.extend(signals);
            } else if child.name().value() == "sync" {
                syncs.push((child.span().offset(), signal(child, "sync")?));
            } else if child.name().value() == "after" {
                shape(child, 1, &["from"])?;
                ensure!(children(child).is_empty(), "after cannot have children");
                let at = arg_time(child, 0)?;
                let from = string(child.get("from").context("after requires from")?)?;
                ensure!(at > 0, "after must be positive");
                ensure!(from != id, "Timed phase transition cannot target itself");
                self.phase_refs.push(from.clone());
                after.push(super::PhaseAfter { from, at });
            } else if child.name().value() == "sequence" {
                let sequence = self
                    .sequence_node(child)
                    .with_context(|| location(source, child))?;
                ensure!(
                    sequence_ids.insert(sequence.id.clone()),
                    "Duplicate sequence ID {} in phase {id}",
                    sequence.id
                );
                sequences.push(sequence);
            } else {
                body.push(child.clone());
            }
        }
        ensure!(
            !enters.is_empty() || !after.is_empty(),
            "phase requires enter or after"
        );
        let events = self.body(&body, "phase", false, 0, None, source)?;
        let branches = body
            .iter()
            .filter(|node| node.name().value() == "branch")
            .map(|node| {
                self.branches
                    .remove(&arg_string(node, 0)?)
                    .context("Missing parsed branch")
            })
            .collect::<Result<Vec<_>>>()?;
        let loops = self.take_loops(&body)?;
        Ok(Phase {
            id,
            enters,
            events,
            branches,
            loops,
            sequences,
            after,
            syncs,
        })
    }

    fn sequence_node(&mut self, node: &KdlNode) -> Result<super::Sequence> {
        shape(node, 1, &[])?;
        let id = arg_string(node, 0)?;
        let mut starts = Vec::new();
        let mut events = Vec::new();
        let mut syncs = Vec::new();
        for child in children(node) {
            match child.name().value() {
                "on" => starts.push(signal(child, "on")?),
                "event" => events.push(self.event(child, 0, None)?),
                "sync" => syncs.push((child.span().offset(), signal(child, "sync")?)),
                name => bail!("Unknown sequence child {name}"),
            }
        }
        ensure!(
            !starts.is_empty() && !events.is_empty(),
            "sequence requires on and event"
        );
        Ok(super::Sequence {
            id,
            starts,
            events,
            syncs,
        })
    }

    fn branch(
        &mut self,
        node: &KdlNode,
        in_loop: bool,
        every: Option<Tick>,
        source: &str,
    ) -> Result<()> {
        shape(node, 1, &["open", "close"])?;
        let id = self.id(node)?;
        let open = property_time(node, "open")?.context("branch requires open")?;
        let close = property_time(node, "close")?.context("branch requires close")?;
        ensure!(open < close, "branch needs open < close");
        if let Some(every) = every {
            ensure!(close <= every, "branch close exceeds iteration");
        }
        let mut options = BTreeSet::new();
        let mut parsed_options = Vec::new();
        let mut merge = None;
        for child in children(node) {
            match child.name().value() {
                "option" => {
                    parsed_options.push(
                        self.option_node(child, &id, close, every, source, &mut options)
                            .with_context(|| location(source, child))?,
                    );
                }
                "merge" => {
                    ensure!(
                        merge.is_none(),
                        "Only one merge per branch at {}",
                        location(source, child)
                    );
                    merge = Some(
                        self.merge_node(child, in_loop, every, source)
                            .with_context(|| location(source, child))?,
                    );
                }
                name => bail!("Unknown branch child {name} at {}", location(source, child)),
            }
        }
        ensure!(options.len() >= 2, "branch needs at least two options");
        validate_option_overlaps(node, open, close)?;
        self.options.insert(id.clone(), options);
        self.branches.insert(
            id.clone(),
            Branch {
                id,
                open,
                close,
                options: parsed_options,
                merge,
            },
        );
        Ok(())
    }

    fn loop_node(&mut self, node: &KdlNode, source: &str) -> Result<()> {
        shape(node, 1, &["start", "every", "count"])?;
        let id = self.id(node)?;
        let start = property_time(node, "start")?.context("loop requires start")?;
        let every = property_time(node, "every")?.context("loop requires every")?;
        ensure!(every > 0, "loop every must be positive");
        let count = node
            .get("count")
            .map(|value| {
                value
                    .as_integer()
                    .and_then(|count| i64::try_from(count).ok())
                    .filter(|&count| count > 0)
                    .context("loop count must be a positive integer")
            })
            .transpose()?;
        let mut body = Vec::new();
        let mut untils = Vec::new();
        let mut syncs = Vec::new();
        for child in children(node) {
            match child.name().value() {
                "sync" => syncs.push((child.span().offset(), signal(child, "sync")?)),
                "until" => {
                    untils.push(signal(child, "until")?);
                }
                _ => body.push(child.clone()),
            }
        }
        let events = self.body(&body, "loop", true, 0, Some(every), source)?;
        check_schedule_syncs(&syncs, 0, Some(every))?;
        let branches = body
            .iter()
            .filter(|node| node.name().value() == "branch")
            .map(|node| {
                self.branches
                    .remove(&arg_string(node, 0)?)
                    .context("Missing parsed branch")
            })
            .collect::<Result<Vec<_>>>()?;
        self.loops.insert(
            id.clone(),
            Loop {
                syncs,
                order: node.span().offset(),
                id,
                start,
                every,
                count,
                untils,
                events,
                branches,
            },
        );
        Ok(())
    }

    fn take_loops(&mut self, nodes: &[KdlNode]) -> Result<Vec<Loop>> {
        nodes
            .iter()
            .filter(|node| node.name().value() == "loop")
            .map(|node| {
                self.loops
                    .remove(&arg_string(node, 0)?)
                    .context("Missing parsed loop")
            })
            .collect()
    }

    fn references(&self) -> Result<()> {
        for phase in &self.phase_refs {
            ensure!(
                self.ids.get("phase").is_some_and(|ids| ids.contains(phase)),
                "Unknown phase in from: {phase}"
            );
        }
        let mut edges = BTreeMap::<&str, Vec<&str>>::new();
        for (branch, target, option) in &self.requires {
            ensure!(
                branch != target,
                "requires cannot reference its own branch: {branch}"
            );
            ensure!(
                self.options
                    .get(target)
                    .is_some_and(|ids| ids.contains(option)),
                "Unknown requires target {target}/{option}"
            );
            edges.entry(branch).or_default().push(target);
        }
        fn visit<'a>(
            id: &'a str,
            edges: &BTreeMap<&'a str, Vec<&'a str>>,
            stack: &mut BTreeSet<&'a str>,
            done: &mut BTreeSet<&'a str>,
        ) -> Result<()> {
            if done.contains(id) {
                return Ok(());
            }
            ensure!(stack.insert(id), "Circular requires reference at {id}");
            for next in edges.get(id).into_iter().flatten() {
                visit(next, edges, stack, done)?;
            }
            stack.remove(id);
            done.insert(id);
            Ok(())
        }
        let mut done = BTreeSet::new();
        for id in edges.keys() {
            visit(id, &edges, &mut BTreeSet::new(), &mut done)?;
        }
        Ok(())
    }
}

pub(crate) fn parse_with_file(source: &str, file: &str) -> Result<Timeline> {
    let mut timeline = parse(source)?;
    for event in timeline.catalog.events.values_mut() {
        event.source_span.file = file.into();
    }
    Ok(timeline)
}

pub(crate) fn parse(source: &str) -> Result<Timeline> {
    let mut declarations = BTreeMap::new();
    let mut choices = Vec::new();
    let doc = expand::document(source, &mut declarations, &mut choices)?;
    let [root] = doc.nodes() else {
        bail!("Expected one timeline root");
    };
    ensure!(
        root.name().value() == "timeline",
        "Expected timeline root, not {}",
        root.name().value()
    );
    shape(root, 2, &[])?;
    ensure!(
        args(root).first().and_then(|v| v.as_integer()) == Some(2),
        "Unsupported timeline version; expected 2"
    );
    let name = arg_string(root, 1)?;
    let mut v = Validation::default();
    let mut timeline = Timeline {
        choices,
        name,
        lifecycle: BTreeMap::new(),
        events: Vec::new(),
        syncs: Vec::new(),
        phases: Vec::new(),
        event_count: 0,
        catalog: super::Catalog::default(),
    };
    for node in children(root) {
        (|| -> Result<()> {
            match node.name().value() {
                role @ ("start" | "wipe" | "boss-complete" | "finish" | "zone-exit") => {
                    let signal = signal(node, role)?;
                    v.phase_refs.extend(signal.from.iter().cloned());
                    timeline
                        .lifecycle
                        .entry(role.into())
                        .or_default()
                        .push(signal);
                }
                "event" => timeline.events.push(v.event(node, 0, None)?),
                "sync" => timeline
                    .syncs
                    .push((node.span().offset(), signal(node, "sync")?)),
                "phase" => {
                    timeline.phases.push(v.phase_node(node, source)?);
                }
                name => bail!("Unknown timeline child {name}"),
            }
            Ok(())
        })()
        .with_context(|| location(source, node))?;
    }
    v.references()?;
    let mut timed_sources = BTreeSet::new();
    for phase in &timeline.phases {
        for after in &phase.after {
            ensure!(
                timed_sources.insert(after.from.as_str()),
                "Duplicate timed transition from phase {}",
                after.from
            );
        }
    }
    timeline.event_count = v.event_count;
    order_events(&mut timeline);
    timeline.catalog = super::Catalog::from_timeline(&timeline, source, &declarations);
    Ok(timeline)
}

fn order_events(timeline: &mut Timeline) {
    fn body<'a>(
        events: &'a mut [Event],
        branches: &'a mut [Branch],
        loops: &'a mut [Loop],
        result: &mut Vec<&'a mut Event>,
    ) {
        result.extend(events);
        for branch in branches {
            for option in &mut branch.options {
                body(&mut option.events, &mut [], &mut option.loops, result);
            }
            if let Some(merge) = &mut branch.merge {
                result.extend(&mut merge.events);
            }
        }
        for repeat in loops {
            body(&mut repeat.events, &mut repeat.branches, &mut [], result);
        }
    }
    let mut events = Vec::new();
    events.extend(&mut timeline.events);
    for phase in &mut timeline.phases {
        body(
            &mut phase.events,
            &mut phase.branches,
            &mut phase.loops,
            &mut events,
        );
        for reaction in &mut phase.sequences {
            events.extend(&mut reaction.events);
        }
    }
    events.sort_by_key(|event| (event.source_start, event.order));
    for (order, event) in events.into_iter().enumerate() {
        event.order = order;
    }
}

pub(crate) fn expanded(source: &str) -> Result<String> {
    parse(source)?;
    let mut document: kdl::KdlDocument = source.parse()?;
    let root = document
        .nodes_mut()
        .first_mut()
        .context("Missing timeline root")?;
    expand::event_syncs(root)?;
    document.clear_format_recursive();
    document.autoformat();
    Ok(document.to_string())
}

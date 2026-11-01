use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use super::{Event, Group, Pull, Signal, alignment, batches, interval, trace};

fn censor_timing(
    row: &mut Value,
    event: &Event,
    origin: &Event,
    at: i64,
    pull: &Pull,
) -> Result<()> {
    let status = row
        .get("status")
        .and_then(Value::as_str)
        .context("Missing check status")?
        .to_owned();
    if status != "unobserved" && !(pull.kill && status == "mismatch") {
        return Ok(());
    }
    if event
        .samples
        .keys()
        .any(|trace| !origin.samples.contains_key(trace))
    {
        let fields = row.as_object_mut().context("Invalid event check")?;
        fields.insert("status".into(), "unresolved".into());
        fields.insert(
            "reason".into(),
            "partial-trace samples lack comparison-origin timings".into(),
        );
        return Ok(());
    }
    let earliest = event
        .samples
        .iter()
        .map(|(trace, time)| Ok(time - origin.samples.get(trace).context("Missing anchor sample")?))
        .collect::<Result<Vec<i64>>>()?
        .into_iter()
        .min()
        .context("Missing timing samples")?;
    // A stopped sequence cannot excuse a missing cast nominally due before the recording ends.
    if status == "unobserved" && at + earliest <= pull.end_ms {
        let fields = row.as_object_mut().context("Invalid event check")?;
        fields.insert("status".into(), "unresolved".into());
        fields.insert(
            "reason".into(),
            "censored alignment but training timing overlaps observed duration".into(),
        );
    }
    // A faster clear can end before a reference mechanic, e.g. C at 60s after a 45s kill.
    if status == "mismatch" && at + earliest > pull.end_ms {
        let fields = row.as_object_mut().context("Invalid event check")?;
        fields.insert("status".into(), "unobserved".into());
        fields.insert(
            "reason".into(),
            "fight ended before every training timing sample".into(),
        );
    }
    Ok(())
}

fn event_check(
    event: &Event,
    reference: &str,
    model: &[Group<'_>],
    observed: &[Group<'_>],
    aligned: &alignment::AttemptAlignment,
) -> Result<Value> {
    let at = *event
        .samples
        .get(reference)
        .context("Missing reference event sample")?;
    let base = json!({"activity":event.signal,"reference_at_ms":at});
    let group = model.iter().position(|group| {
        group
            .observations
            .iter()
            .any(|row| row.signal == &event.signal && row.at == at)
    });
    let Some(index) = group else {
        return Ok(json!({"event":base,"status":"unresolved","reason":"before comparison anchor"}));
    };
    if aligned.censored_left.contains(&index) {
        return Ok(
            json!({"event":base,"status":"unobserved","reason":"suffix after every optimal endpoint"}),
        );
    }
    if let Some((_, peer)) = aligned.pairs.iter().find(|(i, _)| *i == index) {
        let observation = observed
            .get(*peer)
            .and_then(|group| group.unique(&event.signal));
        if let Some(row) = observation {
            return Ok(json!({"event":base,"status":"matched","at_ms":row.at,"batch":row.batch}));
        }
        return Ok(
            json!({"event":base,"status":"ambiguous","reason":"repeated identity inside uncertainty group"}),
        );
    }
    if aligned.ambiguous_left.contains(&index) {
        return Ok(
            json!({"event":base,"status":"ambiguous","reason":"optimal prefix alignments disagree"}),
        );
    }
    let present = observed
        .iter()
        .flat_map(|group| &group.observations)
        .any(|row| row.signal == &event.signal);
    let status = if aligned.missing_left.contains(&index) && !present {
        "mismatch"
    } else {
        "unresolved"
    };
    Ok(
        json!({"event":base,"status":status,"reason":"reference group absent from every optimal alignment"}),
    )
}

fn align(
    model: &[Group<'_>],
    observed: &[Group<'_>],
    complete: bool,
) -> Result<alignment::AttemptAlignment> {
    let left: Vec<_> = model.iter().map(|group| &group.key).collect();
    let right: Vec<_> = observed.iter().map(|group| &group.key).collect();
    if !complete {
        return alignment::align_prefix(&left, &right);
    }
    // A clear is a closed trace: its missing final C must cost a deletion, never a free suffix.
    let result = alignment::align(&left, &right)?;
    Ok(alignment::AttemptAlignment {
        cost: u32::try_from(left.len() + right.len())? - 2 * result.optimal_matches,
        pairs: result.pairs,
        ambiguous_left: result.ambiguous_left,
        missing_left: result.reference_only,
        peer_only: result.peer_only,
        ambiguous_right: result.ambiguous_right,
        ends: vec![left.len()],
        censored_left: Vec::new(),
    })
}

fn origin_conflicts(
    events: &[&Event],
    table: &BTreeMap<Signal, Vec<i64>>,
    reference: &str,
    reference_at: i64,
    at: i64,
) -> bool {
    events.iter().filter(|event|event.occurrence.is_none()).any(|event| {
        event.samples.get(reference).is_some_and(|time|*time>reference_at)
            && table.get(&event.signal).is_some_and(|times|matches!(times.as_slice(),[other] if *other+super::super::common::ORDER_RESOLUTION_MS<at))
    })
}

fn check(
    pull: &Pull,
    reference: &Pull,
    events: &[&Event],
    include: &impl Fn(&Signal) -> bool,
) -> Result<Value> {
    let reference_id = trace(reference);
    let table = super::super::timing_table(pull);
    let mut anchors: Vec<_> = events
        .iter()
        .filter(|event| event.occurrence.is_none())
        .collect();
    anchors.sort_by_key(|event| event.samples.get(&reference_id));
    let origin = anchors.into_iter().find_map(|event| {
        let [at] = table.get(&event.signal)?.as_slice() else {
            return None;
        };
        Some((*event, *at))
    });
    let Some((origin, at)) = origin else {
        return Ok(json!({"status":"unresolved","reason":"no uniquely observed common anchor"}));
    };
    let reference_at = *origin
        .samples
        .get(&reference_id)
        .context("Missing reference origin")?;
    // A moved anchor must not make earlier observed mechanics look absent, e.g. A,C vs C,A.
    if origin_conflicts(events, &table, &reference_id, reference_at, at) {
        return Ok(
            json!({"status":"unresolved","reason":"common anchor order contradicts reference origin"}),
        );
    }
    // shortcut: defer a scope crossing reported phases until attempt phase segmentation is implemented.
    let model = interval(
        reference,
        reference_at,
        None,
        &origin.signal.source,
        include,
    );
    let observed = interval(pull, at, None, &origin.signal.source, include);
    let (model, observed) = match (model, observed) {
        (Ok(model), Ok(observed)) => (model, observed),
        (Err(reason), _) | (_, Err(reason)) => {
            return Ok(json!({"status":"unresolved","reason":reason}));
        }
    };
    if !alignment::within_limit(model.len(), observed.len()) {
        return Ok(json!({"status":"unresolved","reason":"resource-limit"}));
    }
    let aligned = align(&model, &observed, pull.kill)?;
    let mut checks = Vec::new();
    for event in events {
        // A tail learned only from longer traces is outside this completed reference, not absent.
        if !event.samples.contains_key(&reference_id) {
            checks.push(json!({"event":{"activity":event.signal},"status":"unresolved","reason":"event not observed in completed comparison reference"}));
            continue;
        }
        let mut row = if event.signal == origin.signal {
            json!({"event":{"activity":event.signal,"reference_at_ms":reference_at},"status":"matched","at_ms":at,
                "batch":pull.batches.iter().position(|batch|batch.at_ms==at)})
        } else {
            event_check(event, &reference_id, &model, &observed, &aligned)?
        };
        censor_timing(&mut row, event, origin, at, pull)?;
        checks.push(row);
    }
    Ok(
        json!({"status":"evaluated","anchor":origin.signal,"anchor_at_ms":at,"reference_anchor_at_ms":reference_at,
        "edit_cost":aligned.cost,"reference_group_endpoints":aligned.ends,"events":checks,
        "peer_only":batches(&observed,&aligned.peer_only),"ambiguous_peer":batches(&observed,&aligned.ambiguous_right)}),
    )
}

// Check withheld observations separately from training and runtime signal replay.
pub(super) fn checks<'a>(
    pulls: impl Iterator<Item = &'a Pull>,
    reference: &Pull,
    events: &[Event],
    include: &impl Fn(&Signal) -> bool,
) -> Result<Vec<Value>> {
    let mut scopes = BTreeMap::<_, Vec<_>>::new();
    for event in events {
        scopes.entry(&event.signal.source).or_default().push(event);
    }
    let mut result = Vec::new();
    for pull in pulls {
        let mut checks = Vec::new();
        for (source, events) in &scopes {
            checks.push(json!({"source":source,"result":check(pull,reference,events,include)?}));
        }
        let method = if pull.kill {
            "complete-reference-alignment-v1"
        } else {
            "censored-reference-prefix-v1"
        };
        let training = events
            .iter()
            .any(|event| event.samples.contains_key(&trace(pull)));
        result.push(json!({"method":method,"trace":trace(pull),"reference":trace(reference),
            "training":training,"scopes":checks,"not_checked":["KDL schedule timing","ACT signal equivalence","game mechanic identity"]}));
    }
    Ok(result)
}

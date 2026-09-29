use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::Path,
};

use anyhow::{Context, Result};
use serde_json::Value;

fn text_field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {key}"))
}

fn number_field(value: &Value, key: &str) -> Result<i64> {
    value
        .get(key)
        .and_then(Value::as_i64)
        .with_context(|| format!("Missing {key}"))
}

fn array_field<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .with_context(|| format!("Missing {key}"))
}

fn reason_label(reason: &str) -> &str {
    match reason {
        "another cast matches within the default sync window" => {
            "기본 sync 창에서 다른 cast와 충돌"
        }
        "source name cannot be rendered safely as a sync" => "source 이름을 sync로 표현할 수 없음",
        _ => reason,
    }
}

fn cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
}

pub(super) fn render(report: &Value) -> Result<String> {
    let input = report.get("input").context("Missing input")?;
    let validation = report.get("validation").context("Missing validation")?;
    let conflicts = array_field(report, "syncConflicts")?;
    let occurrences = array_field(report, "occurrences")?;
    let slots = array_field(report, "slots")?;
    let collapsed = match report.get("collapsedCasts") {
        Some(_) => array_field(report, "collapsedCasts")?,
        None => &[],
    };
    let mut collapsed_sizes = BTreeMap::new();
    let mut omitted_total = 0;
    for group in collapsed {
        let omitted = array_field(group, "omittedEventIndices")?.len();
        omitted_total += omitted;
        collapsed_sizes.insert(number_field(group, "representativeEventIndex")?, omitted);
    }
    let actors = report
        .get("actorNames")
        .and_then(Value::as_object)
        .context("Missing actorNames")?;
    let abilities = report
        .get("abilityNames")
        .and_then(Value::as_object)
        .context("Missing abilityNames")?;
    let rows: BTreeMap<i64, &Value> = occurrences
        .iter()
        .map(|row| Ok((number_field(row, "event_index")?, row)))
        .collect::<Result<_>>()?;
    let mut out = String::new();
    writeln!(
        out,
        "# {} · fight {}",
        cell(text_field(input, "report")?),
        number_field(input, "fight")?
    )?;
    writeln!(out, "\n| 항목 | 값 |\n| --- | --- |")?;
    if let Some(name) = input.get("name").and_then(Value::as_str) {
        writeln!(out, "| 전투 | {} |", cell(name))?;
    }
    writeln!(out, "| 원본 | {} |", cell(text_field(input, "file")?))?;
    writeln!(
        out,
        "| revision / logVersion | {} / {} |",
        number_field(input, "revision")?,
        number_field(input, "logVersion")?
    )?;
    writeln!(
        out,
        "| 종료 | {} ({} ms) |",
        if report.get("kill").and_then(Value::as_bool) == Some(true) {
            "kill"
        } else {
            "wipe"
        },
        number_field(report, "endMs")?
    )?;
    writeln!(out, "| 표시할 완료 cast 행 | {} |", slots.len())?;
    writeln!(out, "| 대표 행에 묶은 동시 cast | {omitted_total} |")?;
    writeln!(out, "| sync 비활성화 | {} |", conflicts.len())?;
    writeln!(out, "\n## 검증 상태\n")?;
    writeln!(out, "| 검사 | 결과 |\n| --- | --- |")?;
    for (key, label) in [
        ("schemaAndSemantic", "Schema · semantic"),
        ("replay", "원본 재생"),
        ("cactbotParser", "cactbot parser"),
        ("runtime", "runtime"),
    ] {
        let passed = validation
            .get(key)
            .and_then(Value::as_bool)
            .with_context(|| format!("Missing validation.{key}"))?;
        writeln!(
            out,
            "| {label} | {} |",
            if passed { "통과" } else { "미실행" }
        )?;
    }
    writeln!(out, "\n## sync 검토\n")?;
    if conflicts.is_empty() {
        writeln!(out, "비활성화된 sync가 없습니다.")?;
    } else {
        writeln!(
            out,
            "같은 조건의 다른 cast가 기본 ±2.5초 창에 들어오거나 source 이름을 안전하게 표현할 수 없어 비활성화한 행입니다. 전체 occurrence와 충돌 index는 짝이 되는 JSON report에 있습니다.\n"
        )?;
        type Key = (i64, i64, i64, String);
        type Counts = (i64, usize, usize);
        let mut groups: BTreeMap<Key, Counts> = BTreeMap::new();
        for conflict in conflicts {
            let index = number_field(conflict, "eventIndex")?;
            let row = rows
                .get(&index)
                .context("Conflict event not found in occurrences")?;
            let key = (
                number_field(row, "relative_ms")?,
                number_field(row, "ability_id")?,
                number_field(row, "actor_id")?,
                text_field(conflict, "reason")?.to_owned(),
            );
            let count = array_field(conflict, "conflictingEventIndices")?.len();
            let rows_in_group = 1 + collapsed_sizes.get(&index).copied().unwrap_or(0);
            groups
                .entry(key)
                .and_modify(|group| {
                    group.1 += rows_in_group;
                    group.2 = group.2.max(count);
                })
                .or_insert((index, rows_in_group, count));
        }
        writeln!(
            out,
            "같은 시각·능력·source의 cast는 대표 행으로 묶었습니다. 행 수는 원본 cast 수입니다. 대표 index로 JSON report에서 전체 참조를 확인할 수 있습니다.\n"
        )?;
        writeln!(
            out,
            "| 시각 (초) | 능력 | source | 대표 index | 행 수 | 최대 충돌 수 | 이유 |\n| ---: | --- | --- | ---: | ---: | ---: | --- |"
        )?;
        for ((at, ability_id, actor_id, reason), (index, count, collisions)) in groups {
            let ability = abilities
                .get(&ability_id.to_string())
                .and_then(Value::as_str)
                .context("Missing ability name")?;
            let actor = actors
                .get(&actor_id.to_string())
                .and_then(Value::as_str)
                .context("Missing actor name")?;
            writeln!(
                out,
                "| {:.3} | {} ({ability_id:X}) | {} | {index} | {count} | {collisions} | {} |",
                at as f64 / 1000.0,
                cell(ability),
                cell(actor),
                cell(reason_label(&reason))
            )?;
        }
    }
    Ok(out)
}

pub fn markdown_file(input: impl AsRef<Path>) -> Result<()> {
    let input = input.as_ref();
    let output = input.with_extension("md");
    let report: Value = serde_json::from_slice(&fs::read(input)?)?;
    let markdown = render(&report)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .with_context(|| format!("Creating {}", output.display()))?;
    file.write_all(markdown.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

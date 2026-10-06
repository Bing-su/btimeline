use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{GenerateMode, GroupKey, Occurrence};

// Keep report keys explicit while preserving source references, e.g. eventIndices stays an array.
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimeStatistics<T = f64> {
    pub median_ms: T,
    pub min_ms: i64,
    pub max_ms: i64,
    pub sample_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Validation {
    pub schema_and_semantic: bool,
    pub replay: bool,
    pub cactbot_parser: bool,
    pub runtime: bool,
}

impl Default for Validation {
    fn default() -> Self {
        Self {
            schema_and_semantic: true,
            replay: false,
            cactbot_parser: false,
            runtime: false,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BossSegment {
    pub actor_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReportInput<'a> {
    pub file: &'a str,
    pub sha256: &'a str,
    pub report: &'a str,
    pub fight: i64,
    pub name: &'a str,
    pub revision: i64,
    pub game_version: i64,
    pub log_version: i64,
    pub complete: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CollapsedCast {
    pub representative_event_index: usize,
    pub omitted_event_indices: Vec<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SingleSlot {
    pub event_index: usize,
    pub sample_count: usize,
    pub time_ms: i64,
    pub block: usize,
    pub time: TimeStatistics<i64>,
    pub evidence: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SingleBlock {
    pub id: usize,
    pub entry: &'static str,
    pub time: TimeStatistics<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SingleReport<'a> {
    pub status: &'static str,
    pub mode: GenerateMode,
    pub boss_segments: Vec<BossSegment>,
    pub input: ReportInput<'a>,
    pub group: &'a GroupKey,
    pub kill: bool,
    pub end_ms: i64,
    pub occurrences: &'a [Occurrence],
    pub actor_names: BTreeMap<i64, String>,
    pub ability_names: BTreeMap<i64, String>,
    pub emitted_event_indices: Vec<usize>,
    pub collapsed_casts: Vec<CollapsedCast>,
    pub slots: Vec<SingleSlot>,
    pub blocks: Vec<SingleBlock>,
    pub sync_conflicts: Vec<super::draft::SyncConflict>,
    pub validation: Validation,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Sample<'a> {
    pub file: &'a str,
    pub event_indices: &'a [usize],
    pub instance_ids: &'a [i64],
    pub ability_id: i64,
    pub time_ms: i64,
    pub block_entry_ms: i64,
    pub relative_ms: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MultiBlock<'a> {
    pub id: usize,
    pub entry: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<usize>,
    pub draft_entry_ms: serde_json::Number,
    pub time: TimeStatistics,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<Sample<'a>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MultiSlot<'a> {
    pub id: usize,
    pub block: usize,
    pub at_ms: f64,
    pub time: TimeStatistics,
    pub absolute_time: TimeStatistics,
    pub ability_ids: Vec<i64>,
    pub samples: Vec<Sample<'a>>,
    pub unobserved_after_wipe: Vec<&'a str>,
    pub evidence: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OmittedSignal<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<&'a str>,
    pub event_indices: &'a [usize],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<Sample<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<TimeStatistics>,
    pub reason: &'static str,
    pub evidence: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConflictingEvent<'a> {
    pub file: &'a str,
    pub event_index: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MultiConflict<'a> {
    pub slot: usize,
    pub reason: &'static str,
    pub conflicting_events: Vec<ConflictingEvent<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OutputCoverage<'a> {
    pub file: &'a str,
    pub represented_event_indices: std::collections::BTreeSet<usize>,
    pub omitted_event_indices: Vec<usize>,
}

#[derive(Serialize)]
pub(super) struct ObservedPath<'a> {
    pub file: &'a str,
    pub occurrences: &'a [Occurrence],
}

#[derive(Serialize)]
pub(super) struct SensitivePair<'a> {
    pub left: &'a str,
    pub right: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Alignment<'a> {
    pub implementation: &'static str,
    pub order_sensitive_pairs: Vec<SensitivePair<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MultiReport<'a> {
    pub status: &'static str,
    pub mode: GenerateMode,
    pub group: GroupKey,
    pub inputs: Vec<&'a Value>,
    pub blocks: Vec<MultiBlock<'a>>,
    pub slots: Vec<MultiSlot<'a>>,
    pub sync_conflicts: Vec<MultiConflict<'a>>,
    pub omitted_signals: Vec<OmittedSignal<'a>>,
    pub output_coverage: Vec<OutputCoverage<'a>>,
    pub observed_paths: Vec<ObservedPath<'a>>,
    pub unobserved_combinations: &'static str,
    pub alignment: Alignment<'a>,
    pub limitations: [&'static str; 4],
    pub validation: Validation,
}

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
    if report.get("inputs").is_some() {
        return render_multi(report);
    }
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

fn render_multi(report: &Value) -> Result<String> {
    let inputs = array_field(report, "inputs")?;
    let slots = array_field(report, "slots")?;
    let conflicts = array_field(report, "syncConflicts")?;
    let mut out = String::from("# 다중 로그 초안\n\n");
    writeln!(out, "| 항목 | 값 |\n| --- | --- |")?;
    writeln!(out, "| 상태 | draft |")?;
    writeln!(out, "| 모드 | {} |", text_field(report, "mode")?)?;
    writeln!(out, "| 입력 pull | {} |", inputs.len())?;
    writeln!(out, "| 표시할 완료 cast 행 | {} |", slots.len())?;
    writeln!(out, "| sync 비활성화 | {} |", conflicts.len())?;
    writeln!(
        out,
        "\n## 입력\n\n| 전투 | 원본 | report / fight | revision / logVersion | 종료 |\n| --- | --- | --- | --- | --- |"
    )?;
    for item in inputs {
        let input = item.get("input").context("Missing pull input")?;
        writeln!(
            out,
            "| {} | {} | {} / {} | {} / {} | {} ({} ms) |",
            cell(text_field(input, "name")?),
            cell(text_field(input, "file")?),
            cell(text_field(input, "report")?),
            number_field(input, "fight")?,
            number_field(input, "revision")?,
            number_field(input, "logVersion")?,
            if item["kill"] == true { "kill" } else { "wipe" },
            number_field(item, "endMs")?
        )?;
    }
    writeln!(out, "\n## 검증 상태\n\n| 검사 | 결과 |\n| --- | --- |")?;
    for (key, label) in [
        ("schemaAndSemantic", "Schema · semantic"),
        ("replay", "원본 재생"),
        ("cactbotParser", "cactbot parser"),
        ("runtime", "runtime"),
    ] {
        let passed = report
            .get("validation")
            .and_then(|validation| validation.get(key))
            .context("Missing validation result")?
            .as_bool()
            .context("Missing validation result")?;
        writeln!(
            out,
            "| {label} | {} |",
            if passed { "통과" } else { "미실행" }
        )?;
    }
    writeln!(
        out,
        "\n## 블록 진입\n\n| 블록 | 기준 | 중앙값 (ms) | min / max (ms) | 표본 |\n| ---: | --- | ---: | --- | ---: |"
    )?;
    for block in array_field(report, "blocks")? {
        let time = &block["time"];
        writeln!(
            out,
            "| {} | {} | {} | {} / {} | {} |",
            number_field(block, "id")?,
            text_field(block, "entry")?,
            time["medianMs"],
            time["minMs"],
            time["maxMs"],
            time["sampleCount"]
        )?;
    }
    writeln!(
        out,
        "\n## 슬롯 시간\n\n블록 진입 이후의 관측 밀리초 통계입니다. 원본 index·instance·wipe 미관측 참조는 JSON report에 보존합니다.\n"
    )?;
    writeln!(
        out,
        "| 슬롯 | 블록 | 능력 ID | 중앙값 (ms) | min / max (ms) | 표본 | 근거 |\n| ---: | ---: | --- | ---: | --- | ---: | --- |"
    )?;
    for slot in slots {
        let time = &slot["time"];
        let ids = array_field(slot, "abilityIds")?
            .iter()
            .map(|id| {
                id.as_i64()
                    .map(|id| format!("{id:X}"))
                    .context("Missing ability ID")
            })
            .process_results(|mut ids| ids.join(" / "))?;
        writeln!(
            out,
            "| {} | {} | {} | {} | {} / {} | {} | {} |",
            number_field(slot, "id")?,
            number_field(slot, "block")?,
            ids,
            time["medianMs"],
            time["minMs"],
            time["maxMs"],
            time["sampleCount"],
            text_field(slot, "evidence")?
        )?;
    }
    writeln!(
        out,
        "\n## sync 검토\n\n| 슬롯 | 이유 | 충돌 수 |\n| ---: | --- | ---: |"
    )?;
    for conflict in conflicts {
        writeln!(
            out,
            "| {} | {} | {} |",
            number_field(conflict, "slot")?,
            cell(reason_label(text_field(conflict, "reason")?)),
            array_field(conflict, "conflictingEvents")?.len()
        )?;
    }
    if let Some(extension) = report.get("extensions") {
        writeln!(out, "\n## 분기·페이즈 확장\n\n| 항목 | 값 |\n| --- | --- |")?;
        writeln!(out, "| 후보 채택 | {} |", extension["accepted"])?;
        writeln!(
            out,
            "| 독립 lookahead (ms) | {} |",
            extension["lookaheadMs"]
        )?;
        writeln!(out, "| 실제 runtime 표시 | 미실행 |")?;
        if let Some(reason) = extension.get("reason").and_then(Value::as_str) {
            writeln!(out, "| 거부 이유 | {} |", cell(reason))?;
        }
        if extension["accepted"] == true {
            writeln!(
                out,
                "\n| 분기 | 경로 label | 진입 슬롯 | 가상 진입 / 합류 (ms) | window before / after (ms) |\n| ---: | --- | ---: | --- | --- |"
            )?;
            for branch in array_field(extension, "branches")? {
                for path in array_field(branch, "paths")? {
                    let selector = number_field(path, "selectorSlot")?;
                    let slot = slots
                        .get(usize::try_from(selector)?)
                        .context("Missing selector slot")?;
                    writeln!(
                        out,
                        "| {} | {} | {selector} | {} / {} | {} / {} |",
                        branch["id"],
                        cell(text_field(path, "label")?),
                        path["entryMs"],
                        branch["mergeMs"],
                        slot.pointer("/windowMs/0")
                            .context("Missing before window")?,
                        slot.pointer("/windowMs/1")
                            .context("Missing after window")?
                    )?;
                }
            }
            writeln!(
                out,
                "\n| 페이즈 label | 전환 슬롯 | 가상 진입 (ms) | 보정 시계 min / max (ms) | window before / after (ms) |\n| --- | ---: | ---: | --- | --- |"
            )?;
            for phase in array_field(extension, "phases")? {
                let selector = number_field(phase, "selectorSlot")?;
                let slot = slots
                    .get(usize::try_from(selector)?)
                    .context("Missing phase selector")?;
                writeln!(
                    out,
                    "| {} | {selector} | {} | {} / {} | {} / {} |",
                    cell(text_field(phase, "label")?),
                    phase["entryMs"],
                    slot.pointer("/clockTime/minMs")
                        .context("Missing minimum clock")?,
                    slot.pointer("/clockTime/maxMs")
                        .context("Missing maximum clock")?,
                    slot.pointer("/windowMs/0")
                        .context("Missing before window")?,
                    slot.pointer("/windowMs/1")
                        .context("Missing after window")?
                )?;
            }
            writeln!(
                out,
                "\n채택한 확장은 모든 train pull의 원본 cast/begincast로 검사했습니다. 예고 목록과 실제 jump는 JSON의 `extensions.checks[].previews`·`jumps`에 별도로 기록합니다.\n"
            )?;
        }
    }
    writeln!(out, "\n## 제한\n\n| 항목 | 값 |\n| --- | --- |")?;
    writeln!(
        out,
        "| 참조 경로에서 제외한 신호 | {} |",
        array_field(report, "omittedSignals")?.len()
    )?;
    writeln!(
        out,
        "| 방향 민감한 로그 쌍 | {} |",
        array_field(&report["alignment"], "orderSensitivePairs")?.len()
    )?;
    for limitation in array_field(report, "limitations")? {
        writeln!(
            out,
            "| 제한 | {} |",
            cell(limitation.as_str().context("Missing limitation")?)
        )?;
    }
    Ok(out)
}

pub fn markdown_file(input: impl AsRef<Path>) -> Result<()> {
    let input = input.as_ref();
    let output = input.with_extension("md");
    let report: Value = serde_json::from_slice(&fs::read(input)?)?;
    let markdown = render(&report)?;
    crate::output::write_new(&[(&output, markdown.as_bytes())])
}

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const digest = (value: string | Buffer) => createHash("sha256").update(value).digest("hex");
const read = (file: string) => JSON.parse(fs.readFileSync(file, "utf8"));

export class InputExclusion extends Error {
  constructor(
    message: string,
    readonly evidence: Record<string, unknown>,
  ) {
    super(message);
    this.name = "InputExclusion";
  }
}

function requireInput(
  condition: unknown,
  message: string,
  evidence: Record<string, unknown>,
): asserts condition {
  if (!condition) throw new InputExclusion(message, evidence);
}

// Exclude unusable inputs from the score; execution errors and actual prediction failures still fail evaluation.
export function holdoutSummary(traces: { status: string; passed: boolean | null }[]) {
  const eligible = traces.filter((trace) => trace.status !== "excluded");
  return {
    selected: traces.length,
    excluded: traces.length - eligible.length,
    eligible: eligible.length,
    passed_traces: eligible.filter((trace) => trace.passed === true).length,
    passed: eligible.length > 0 && eligible.every((trace) => trace.passed === true),
  };
}

// Keep all cast observations; select evaluation targets independently so filtering cannot hide competing syncs.
export function griffinRecording(raw: any, names: Record<string, string>) {
  const actors = new Map<number, string>(
    raw.report.masterData.actors.map((actor: any) => [actor.id, actor.name]),
  );
  const origin = raw.events[0].timestamp;
  const observations = raw.events.flatMap((item: any, index: number) => {
    if (item.type !== "cast") return [];
    const source = actors.get(item.sourceID);
    requireInput(source, `Unknown cast source ${item.sourceID}`, {
      raw_index: index,
      timestamp_ms: item.timestamp,
      source_id: item.sourceID,
    });
    return [
      {
        at_ms: item.timestamp - origin,
        type: "Ability",
        fields: { id: item.abilityGameID.toString(16).toUpperCase(), source },
        raw_index: index,
      },
    ];
  });
  const entry = observations.find(
    (item: any) => item.fields.id === "B0EB" && item.fields.source === "Thundergust Griffin",
  );
  requireInput(entry, "Missing Griffin entry", { events: raw.events.length });
  const completion = raw.events.findIndex(
    (item: any) =>
      item.timestamp >= origin + entry.at_ms &&
      item.type === "damage" &&
      actors.get(item.targetID) === "Thundergust Griffin" &&
      item.targetResources?.hitPoints === 0,
  );
  requireInput(completion >= 0, "Missing observed Griffin completion", {
    entry_event: entry.raw_index,
    last_damage_event: raw.events.findLastIndex(
      (item: any) => item.type === "damage" && actors.get(item.targetID) === "Thundergust Griffin",
    ),
  });
  const end_ms = raw.events[completion].timestamp - origin;
  const targets = observations.filter(
    (item: any) =>
      item.at_ms >= entry.at_ms &&
      item.at_ms <= end_ms &&
      names[item.fields.id] &&
      item.fields.source === (item.fields.id === "B0F4" ? "Shock Storm" : "Thundergust Griffin"),
  );
  const samples = targets.map((item: any) => [
    item.at_ms - entry.at_ms,
    item.fields.id,
    item.raw_index,
  ]);
  return {
    origin,
    entry_ms: entry.at_ms,
    completion_event: completion,
    end_ms,
    observations,
    targets,
    samples,
  };
}

function main() {
  const [binary, directory, output] = process.argv.slice(2);
  assert(
    binary && directory && output,
    "Usage: bun scripts/verify_griffin_holdout.ts BTIMELINE RAW_DIRECTORY REPORT.json",
  );
  const planFile = "tests/fixtures/authored/griffin-holdout-plan.json";
  const plan = read(planFile);
  assert.equal(
    digest(fs.readFileSync(plan.timeline)),
    plan.timeline_sha256,
    "Candidate changed after freeze",
  );
  execFileSync(binary, ["validate", plan.timeline]);
  const development = read(plan.development_fixture);
  const reports = new Set(development.traces.map((trace: any) => trace.trace.split("/")[0]));
  const fingerprints = new Set(
    development.traces.map((trace: any) =>
      digest(JSON.stringify(trace.samples.map(([at, id]: any) => [at, id]))),
    ),
  );
  const identities = new Set();
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "griffin-holdout-"));
  const recording = path.join(temporary, "recording.json");
  const traces = [];
  try {
    for (const fight of plan.fights) {
      const identity = `${fight.report}/${fight.fight}`;
      assert(
        !reports.has(fight.report) && !identities.has(identity),
        `Duplicate report/fight ${identity}`,
      );
      identities.add(identity);
      const input = path.join(directory, `${fight.report}_${fight.fight}.json`);
      let stage = "input-validation";
      try {
        requireInput(fs.existsSync(input), "Missing input file", { input });
        // A failed inspection command is a tool failure, not evidence that this log can safely be excluded.
        stage = "input-inspection";
        execFileSync(binary, ["inspect", input, "--json"], { stdio: ["ignore", "ignore", "pipe"] });
        stage = "input-validation";
        const raw = read(input);
        requireInput(
          raw.report.code === fight.report &&
            raw.report.fights[0].id === fight.fight &&
            raw.report.fights[0].startTime === fight.startTime &&
            raw.report.fights[0].endTime === fight.endTime,
          "Input differs from frozen fight",
          { expected: fight, actual: raw.report.fights[0] },
        );
        const projected = griffinRecording(raw, plan.targets);
        const fingerprint = digest(
          JSON.stringify(projected.samples.map(([at, id]: any) => [at, id])),
        );
        requireInput(!fingerprints.has(fingerprint), `Duplicate major-cast sequence ${identity}`, {
          fingerprint,
        });
        fingerprints.add(fingerprint);
        stage = "timeline-evaluation";
        const observations = projected.observations
          .filter((item: any) => item.at_ms <= projected.end_ms)
          .map(({ raw_index, ...item }: any) => item);
        fs.writeFileSync(
          recording,
          JSON.stringify({ version: 1, end_ms: projected.end_ms, observations }),
        );
        const replay = JSON.parse(
          execFileSync(binary, ["compare", plan.timeline, recording, "--json"], {
            encoding: "utf8",
          }),
        );
        assert.equal(replay.summaries[recording].status, "replayed");
        const rows = replay.recordings[recording];
        const diagnostics = rows.filter((row: any) =>
          ["ambiguous", "violation", "unresolved"].includes(row.action),
        );
        const phases = rows.filter((row: any) => row.action === "phase");
        const safeEntry =
          phases.length === 1 &&
          phases[0].at_ms === projected.entry_ms &&
          !rows.some((row: any) => row.action === "event" && row.at_ms < projected.entry_ms);
        const targets = projected.targets.map((item: any) => {
          const name = plan.targets[item.fields.id];
          const executed = rows.filter(
            (row: any) => row.action === "event" && row.detail.includes(`: ${name} (visible=true,`),
          );
          const replayError = Math.min(
            ...executed.map((row: any) => Math.abs(row.at_ms - item.at_ms)),
          );
          // Exclude the entry mechanic: no five-second forecast is possible before this timeline starts.
          if (item.at_ms - projected.entry_ms < plan.forecast_lead_ms)
            return {
              raw_index: item.raw_index,
              id: item.fields.id,
              at_ms: item.at_ms,
              replay_error_ms: Number.isFinite(replayError) ? replayError : null,
              forecast: null,
            };
          const forecast = JSON.parse(
            execFileSync(
              binary,
              [
                "forecast",
                plan.timeline,
                recording,
                "--at-ms",
                String(item.at_ms - plan.forecast_lead_ms),
                "--horizon-ms",
                String(plan.horizon_ms),
              ],
              { encoding: "utf8" },
            ),
          );
          const candidates = forecast.events.filter((row: any) => row.name === name);
          const error = Math.min(...candidates.map((row: any) => Math.abs(row.at_ms - item.at_ms)));
          return {
            raw_index: item.raw_index,
            id: item.fields.id,
            at_ms: item.at_ms,
            replay_error_ms: Number.isFinite(replayError) ? replayError : null,
            forecast: {
              error_ms: Number.isFinite(error) ? error : null,
              passed: error <= plan.tolerance_ms,
              diagnostics: [...forecast.diagnostics, ...forecast.projection_diagnostics],
            },
          };
        });
        traces.push({
          status: "evaluated",
          input,
          sha256: digest(fs.readFileSync(input)),
          trace: identity,
          major_cast_fingerprint: fingerprint,
          origin_timestamp_ms: projected.origin,
          entry_ms: projected.entry_ms,
          end_ms: projected.end_ms,
          completion_event: projected.completion_event,
          cast_observations: observations.length,
          safe_entry: safeEntry,
          diagnostics,
          targets,
          passed:
            safeEntry &&
            diagnostics.length === 0 &&
            targets.every(
              (target: any) =>
                target.replay_error_ms !== null &&
                target.replay_error_ms <= plan.tolerance_ms &&
                (!target.forecast ||
                  (target.forecast.passed && target.forecast.diagnostics.length === 0)),
            ),
        });
      } catch (error) {
        // Preserve exclusion reasons separately; a runtime failure must never become an input-quality exclusion.
        traces.push({
          status: error instanceof InputExclusion ? "excluded" : "error",
          input,
          trace: identity,
          sha256: fs.existsSync(input) ? digest(fs.readFileSync(input)) : null,
          passed: error instanceof InputExclusion ? null : false,
          stage,
          error: String(error),
          ...(error instanceof InputExclusion ? { evidence: error.evidence } : {}),
        });
      }
    }
    const summary = holdoutSummary(traces);
    const report = {
      plan: planFile,
      plan_sha256: digest(fs.readFileSync(planFile)),
      timeline_sha256: plan.timeline_sha256,
      role: plan.role,
      projection: plan.projection,
      traces,
      summary,
      passed: summary.passed,
      not_checked: [
        "ACT equivalence and lifecycle signals",
        "UI visibility",
        "unobserved late cycles",
        "non-target ability coverage",
      ],
    };
    fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
    console.log(
      `Held-out Griffin evaluation: ${summary.passed_traces}/${summary.eligible} eligible traces passed; ${summary.excluded}/${summary.selected} inputs excluded; ${output}`,
    );
    assert(
      report.passed,
      "Held-out failures retained in report; do not revise and reuse as independent evaluation",
    );
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

if (import.meta.main) main();

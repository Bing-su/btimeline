import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const [binary, output, baseline] = process.argv.slice(2);
assert(
  binary && output,
  "Usage: bun scripts/verify_authored_griffin.ts BTIMELINE REPORT.json [BASELINE.kdl]",
);
const timeline = "timelines/mistwake-griffin.kdl";
const fixture = "tests/fixtures/authored/griffin-samples.json";
const corpus = JSON.parse(fs.readFileSync(fixture, "utf8"));
const names: Record<string, string> = {
  B0EB: "Thunderspark",
  B0EC: "High Volts",
  B0EF: "Thundering Roar",
  B0F3: "Storm Surge",
  B0F4: "Electrifying Flight",
  B0F5: "Fulgurous Fall",
  B0F8: "Electrogenetic Force",
  B0F9: "Golden Talons",
};
type Sample = [number, string, number];
type Observation = { at_ms: number; type: string; fields: Record<string, string> };
type Row = { action: string; at_ms: number; detail: string; run: number };
const observation = ([at_ms, id]: Sample): Observation => ({
  at_ms,
  type: "Ability",
  fields: { id, source: id === "B0F4" ? "Shock Storm" : "Thundergust Griffin" },
});
const start = observation([0, "B0EB", 0]);
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "griffin-authoring-"));
const recording = path.join(directory, "recording.json");
const sha256 = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");

function replay(file: string, observations: Observation[], end_ms: number): Row[] {
  fs.writeFileSync(recording, JSON.stringify({ version: 1, observations, end_ms }));
  const result = JSON.parse(
    execFileSync(binary, ["compare", file, recording, "--json"], { encoding: "utf8" }),
  );
  assert.equal(result.summaries[recording].status, "replayed", file);
  return result.recordings[recording];
}
function query(
  file: string,
  observations: Observation[],
  end_ms: number,
  at_ms: number,
): {
  events: { name: string; at_ms: number; lead_ms: number }[];
  diagnostics: Row[];
  projection_diagnostics: Row[];
} {
  fs.writeFileSync(recording, JSON.stringify({ version: 1, observations, end_ms }));
  const result = JSON.parse(
    execFileSync(
      binary,
      ["forecast", file, recording, "--at-ms", String(at_ms), "--horizon-ms", "6500"],
      { encoding: "utf8" },
    ),
  );
  assert.equal(result.status, "active", file);
  return result;
}
const clean = (rows: Row[]) =>
  rows.filter((row) => ["ambiguous", "violation", "unresolved"].includes(row.action));
const event = (row: Row, name: string) =>
  row.action === "event" && row.detail.includes(`: ${name} (visible=true,`);

try {
  execFileSync(binary, ["validate", timeline]);
  let rawInputsChecked = 0;
  for (const trace of corpus.traces) {
    if (!fs.existsSync(trace.input)) continue;
    // Validate the compact development fixture against raw indices when local corpus files are available.
    assert.equal(sha256(trace.input), trace.sha256, trace.input);
    const raw = JSON.parse(fs.readFileSync(trace.input, "utf8"));
    const actors = new Map(
      raw.report.masterData.actors.map((actor: any) => [actor.id, actor.name]),
    );
    for (const sample of trace.samples as Sample[]) {
      const original = raw.events[sample[2]];
      assert.equal(original.type, "cast");
      assert.equal(original.timestamp - trace.origin_timestamp_ms, sample[0]);
      assert.equal(original.abilityGameID.toString(16).toUpperCase(), sample[1]);
      assert.equal(actors.get(original.sourceID), observation(sample).fields.source);
    }
    assert.equal(
      raw.events[trace.completion_event].timestamp - trace.origin_timestamp_ms,
      trace.end_ms,
    );
    rawInputsChecked++;
  }
  const reports = [];
  for (const [label, file] of [
    ["authored", timeline],
    ...(baseline ? [["baseline", baseline]] : []),
  ]) {
    for (const trace of corpus.traces) {
      const samples: Sample[] = trace.samples;
      for (const mode of ["start-only", "all-major", "five-second-query"]) {
        // Supply the whole recording to test the cutoff: the query must ignore the target and all subsequent signals.
        const runs =
          mode === "five-second-query"
            ? samples
                .filter((sample) => sample[0] >= 5000)
                .map((sample) => ({
                  targets: [sample],
                  observations: samples.map(observation),
                  end: trace.end_ms,
                }))
            : [
                {
                  targets: samples,
                  observations: mode === "start-only" ? [start] : samples.map(observation),
                  end: trace.end_ms,
                },
              ];
        const misses = [];
        let matched = 0,
          total = 0,
          maxError = 0,
          bad = 0;
        const leads: number[] = [];
        for (const run of runs) {
          const queried =
            mode === "five-second-query"
              ? query(file, run.observations, run.end, run.targets[0][0] - 5000)
              : null;
          const rows = queried ? [] : replay(file, run.observations, run.end);
          bad += queried
            ? queried.diagnostics.length + queried.projection_diagnostics.length
            : clean(rows).length;
          if (label === "authored" && mode !== "five-second-query") {
            // The authored displays correspond only to the selected targets; count extra forecasts as failures too.
            assert.equal(
              rows.filter((row) => row.action === "event").length,
              run.targets.length,
              `${trace.trace}: extra or missing forecast`,
            );
          }
          for (const sample of run.targets) {
            const candidates = queried
              ? queried.events.filter((row) => row.name === names[sample[1]])
              : rows.filter((row) => event(row, names[sample[1]]));
            const error = Math.min(...candidates.map((row) => Math.abs(row.at_ms - sample[0])));
            total++;
            if (error <= 1500) {
              matched++;
              maxError = Math.max(maxError, error);
              if (queried)
                leads.push(
                  ...queried.events
                    .filter(
                      (row) =>
                        row.name === names[sample[1]] && Math.abs(row.at_ms - sample[0]) <= 1500,
                    )
                    .map((row) => row.lead_ms),
                );
            } else
              misses.push({
                at_ms: sample[0],
                id: sample[1],
                error_ms: Number.isFinite(error) ? error : null,
              });
          }
        }
        reports.push({
          label,
          trace: trace.trace,
          mode,
          matched,
          total,
          max_error_ms: matched ? maxError : null,
          ...(leads.length
            ? { min_lead_ms: Math.min(...leads), max_lead_ms: Math.max(...leads) }
            : {}),
          bad,
          misses,
        });
      }
    }
  }
  const cases = [];
  for (const shift of [-5000, 12000]) {
    const at = 94200 + shift;
    const marker = observation([at, "B0F3", 0]);
    const rows = replay(timeline, [start, marker, marker], 176000 + shift);
    assert.deepEqual(clean(rows), []);
    // Late correction preserves processed displays; verify recovery at the next unprocessed mechanic instead of replaying the past.
    const expectedEvents: [string, number][] =
      shift < 0
        ? [
            ["Electrifying Flight", 94800 + shift],
            ["High Volts", 101400 + shift],
            ["Golden Talons", 136000 + shift],
          ]
        : [
            ["Golden Talons", 136000 + shift],
            ["Thunderspark", 171100 + shift],
          ];
    for (const [name, expected] of expectedEvents) {
      const near = rows.filter((row) => event(row, name) && Math.abs(row.at_ms - expected) < 50);
      assert.equal(near.length, 1, `Recovery ${shift}: ${name}`);
    }
    assert.equal(rows.filter((row) => event(row, "Electrifying Flight")).length, 1);
    cases.push({
      case: "early/late recovery with duplicate marker",
      shift_ms: shift,
      status: "passed",
    });
  }
  const wrong = replay(
    timeline,
    [
      start,
      observation([50000, "B0EB", 0]),
      { at_ms: 106200, type: "Ability", fields: { id: "B0F3", source: "Other Boss" } },
    ],
    145000,
  );
  assert.deepEqual(clean(wrong), []);
  assert(wrong.some((row) => event(row, "Golden Talons") && row.at_ms === 136000));
  assert.equal(wrong.filter((row) => row.action === "phase").length, 1);
  cases.push({
    case: "wrong source and earlier reused entry cannot restart or shift progression",
    status: "passed",
  });
  const future = replay(timeline, [start], 400000);
  assert.deepEqual(clean(future), []);
  assert(future.some((row) => event(row, "Electrifying Flight") && row.at_ms > 250000));
  cases.push({
    case: "missing later signals still predict unobserved future cycles",
    status: "passed",
  });
  for (const reset of [
    { type: "InCombat", fields: { inGameCombat: "0" } },
    { type: "ActorControl", fields: { command: "4000000F" } },
    { type: "ChangeZone", fields: {} },
  ]) {
    const rows = replay(
      timeline,
      [start, { at_ms: 95000, ...reset }, observation([120000, "B0EB", 0])],
      140000,
    );
    assert.deepEqual(clean(rows), []);
    assert(!rows.some((row) => row.action === "event" && row.at_ms >= 95000 && row.at_ms < 120000));
    assert(rows.some((row) => event(row, "High Volts") && row.at_ms === 131700 && row.run === 2));
    cases.push({
      case: `${reset.type} cancels pending rotation and allows restart`,
      status: "passed",
    });
  }
  const report = {
    timeline,
    timeline_sha256: sha256(timeline),
    fixture,
    fixture_sha256: sha256(fixture),
    role: corpus.role,
    raw_inputs_checked: rawInputsChecked,
    forecast_lead_ms: 5000,
    tolerance_ms: 1500,
    reports,
    cases,
    ...(baseline ? { baseline, baseline_sha256: sha256(baseline) } : {}),
    not_checked: [
      "independent holdout",
      "ACT equivalence",
      "UI lookahead visibility",
      "minor/unknown ability coverage",
      "full late-cycle repetition beyond observed suffix",
    ],
  };
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  for (const row of reports.filter((row) => row.label === "authored")) {
    assert.equal(row.bad, 0, JSON.stringify(row));
    assert.equal(row.matched, row.total, JSON.stringify(row));
  }
  console.log(
    `Verified ${reports.filter((row) => row.label === "authored").length} development checks and ${cases.length} control cases; report: ${output}`,
  );
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}

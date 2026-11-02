import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { griffinScenarios, type Sample } from "./griffin_scenarios.ts";

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
  events: { id: string; name: string; at_ms: number; lead_ms: number }[];
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
  const continuous = [];
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
    const completion = raw.events[trace.completion_event];
    assert.equal(actors.get(completion.targetID), "Thundergust Griffin");
    assert.equal(completion.targetResources.hitPoints, 0);
    const origin = raw.events[0].timestamp;
    const entry = trace.origin_timestamp_ms - origin;
    const finishAt = completion.timestamp - origin;
    // Include players, trash and prior bosses so preselecting major casts cannot hide a competing sync.
    const casts: Observation[] = raw.events
      .filter((item: any) => item.type === "cast")
      .map((item: any) => {
        const source = actors.get(item.sourceID);
        assert.equal(typeof source, "string", `${trace.trace}: source ${item.sourceID}`);
        return {
          at_ms: item.timestamp - origin,
          type: "Ability",
          fields: { id: item.abilityGameID.toString(16).toUpperCase(), source },
        };
      });
    const end = Math.max(finishAt, casts.at(-1)!.at_ms) + 30000;
    const rows = replay(timeline, casts, end);
    assert.deepEqual(clean(rows), [], trace.trace);
    const phases = rows.filter((row) => row.action === "phase");
    assert.equal(phases.length, 1, trace.trace);
    assert.equal(phases[0].at_ms, entry, trace.trace);
    assert(!rows.some((row) => row.action === "event" && row.at_ms < entry), trace.trace);
    let maxQueryError = 0;
    for (const sample of trace.samples as Sample[]) {
      const at = entry + sample[0];
      assert(rows.some((row) => event(row, names[sample[1]]) && Math.abs(row.at_ms - at) <= 1500));
      if (sample[0] < 5000) continue;
      const forecast = query(timeline, casts, end, at - 5000);
      assert.deepEqual([...forecast.diagnostics, ...forecast.projection_diagnostics], []);
      const error = Math.min(
        ...forecast.events
          .filter((row) => row.name === names[sample[1]])
          .map((row) => Math.abs(row.at_ms - at)),
      );
      assert(error <= 1500, `${trace.trace}: full-cast forecast at ${at}`);
      maxQueryError = Math.max(maxQueryError, error);
    }
    // HP=0 is evidence of the endpoint, not an observed ACT InCombat signal; test cancellation as an explicit assumption.
    const stopped = replay(
      timeline,
      [...casts, { at_ms: finishAt, type: "InCombat", fields: { inGameCombat: "0" } }].sort(
        (left, right) => left.at_ms - right.at_ms,
      ),
      end,
    );
    assert.deepEqual(clean(stopped), [], trace.trace);
    assert.deepEqual(
      stopped.filter((row) => row.action === "event" && row.at_ms < finishAt),
      rows.filter((row) => row.action === "event" && row.at_ms < finishAt),
      trace.trace,
    );
    assert(!stopped.some((row) => row.action === "event" && row.at_ms >= finishAt));
    const extrapolated = rows.filter((row) => row.action === "event" && row.at_ms > finishAt);
    assert(extrapolated.length > 0, `${trace.trace}: missing finish must leave predictions active`);
    continuous.push({
      trace: trace.trace,
      cast_observations: casts.length,
      pre_entry_casts: casts.filter((item) => item.at_ms < entry).length,
      entry_ms: entry,
      targets: trace.samples.length,
      forecast_targets: trace.samples.filter((sample: Sample) => sample[0] >= 5000).length,
      max_query_error_ms: maxQueryError,
      synthetic_finish_ms: finishAt,
      predictions_after_completion_without_finish: extrapolated.length,
      status: "passed",
    });
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
  const recovery = [];
  for (const trace of corpus.traces) {
    for (const scenario of griffinScenarios(trace.samples, trace.end_ms)) {
      const observations = scenario.observations.map(observation);
      const rows = replay(timeline, observations, scenario.end_ms);
      let bad = clean(rows).length;
      const targets = [];
      for (const { sample, event_id } of scenario.targets
        .map((sample, index) => ({ sample, event_id: scenario.event_ids[index] }))
        .filter((row) => row.sample[0] >= scenario.onset_ms)) {
        const projected = query(timeline, observations, scenario.end_ms, sample[0] - 5000);
        bad += projected.diagnostics.length + projected.projection_diagnostics.length;
        const candidates = projected.events.filter((row) => row.id === event_id);
        const nearest = candidates.sort(
          (a, b) => Math.abs(a.at_ms - sample[0]) - Math.abs(b.at_ms - sample[0]),
        )[0];
        const queryError = nearest ? Math.abs(nearest.at_ms - sample[0]) : null;
        const executed = rows.filter(
          (row) => row.action === "event" && row.detail.startsWith(event_id + ":"),
        );
        const replayError = Math.min(...executed.map((row) => Math.abs(row.at_ms - sample[0])));
        targets.push({
          id: sample[1],
          event_id,
          at_ms: sample[0],
          source_event_index: sample[2],
          query_ok: queryError !== null && queryError <= 1500,
          forecast_ms: nearest?.at_ms ?? null,
          query_error_ms: queryError,
          replay_error_ms: Number.isFinite(replayError) ? replayError : null,
        });
      }
      // A single correct display does not prove recovery; require the entire remaining observed suffix to forecast correctly.
      const lastMiss = targets.findLastIndex((row) => !row.query_ok);
      const suffix = targets.slice(lastMiss + 1);
      const restored = suffix.length >= 2 ? suffix[0] : undefined;
      recovery.push({
        trace: trace.trace,
        scenario: scenario.name,
        shift_ms: scenario.shift_ms,
        omitted_signals: scenario.omit,
        onset_ms: scenario.onset_ms,
        scope_end_ms: scenario.end_ms,
        matched: targets.filter((row) => row.query_ok).length,
        total: targets.length,
        stable_recovery_ms: restored?.at_ms ?? null,
        recovery_target: restored?.id ?? null,
        correct_suffix_targets: suffix.length,
        accepted_syncs: rows.filter(
          (row) => row.action === "sync" && row.at_ms >= scenario.onset_ms,
        ),
        bad,
        targets,
      });
    }
  }
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
    continuous: {
      projection:
        "All raw FFLogs casts to Ability, original actor names; HP=0 is not a runtime stop signal",
      finish_assumption:
        "Synthetic InCombat=0 at Griffin HP=0; availability and timing in ACT unverified",
      skipped_inputs: corpus.traces
        .filter((trace: any) => !fs.existsSync(trace.input))
        .map((trace: any) => trace.input),
      traces: continuous,
    },
    forecast_lead_ms: 5000,
    tolerance_ms: 1500,
    reports,
    cases,
    recovery,
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
  for (const row of recovery) {
    assert.equal(row.bad, 0, JSON.stringify(row));
    assert(
      row.matched < row.total,
      "A changed schedule must expose its pre-recovery forecast errors",
    );
    if (row.scenario === "delayed-without-anchors") {
      assert.equal(row.stable_recovery_ms, null);
      assert.equal(row.accepted_syncs.length, 0);
    }
  }
  for (const scenario of ["delayed-without-storm", "delayed-without-storm-or-volts"]) {
    const representative = recovery.find(
      (row) => row.trace === "YQAawtmnD6FRVzPj/80" && row.scenario === scenario,
    );
    assert(representative?.stable_recovery_ms, `Fallback did not restore forecasts: ${scenario}`);
  }
  console.log(
    `Verified ${reports.filter((row) => row.label === "authored").length} development checks, ${continuous.length} full-cast traces and ${cases.length} control cases; evaluated ${recovery.length} perturbations with pre-recovery misses retained; report: ${output}`,
  );
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}

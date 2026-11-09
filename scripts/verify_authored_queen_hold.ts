import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { queenHoldRecording } from "./queen_hold_recording.ts";
import { holdoutSummary, InputExclusion } from "./types.ts";

const [binary, manifestFile, output] = process.argv.slice(2);
assert(
  binary && manifestFile && output,
  "Usage: bun scripts/verify_authored_queen_hold.ts BTIMELINE MANIFEST.json REPORT.json",
);
const hash = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
assert.equal(hash(manifest.timeline), manifest.timeline_sha256, "Candidate changed");
execFileSync(binary, ["validate", manifest.timeline]);
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "queen-hold-evaluation-"));
const recording = path.join(temporary, "recording.json");
const traces = [];
const fingerprints = new Set();
try {
  for (const item of manifest.inputs) {
    let stage = "input-validation";
    try {
      if (!fs.existsSync(item.input))
        throw new InputExclusion("Missing input", { input: item.input });
      assert.equal(hash(item.input), item.sha256, "Input changed since selection");
      stage = "input-inspection";
      execFileSync(binary, ["inspect", item.input, "--json"], {
        stdio: ["ignore", "ignore", "pipe"],
      });
      stage = "input-validation";
      const raw = JSON.parse(fs.readFileSync(item.input, "utf8"));
      const projected = queenHoldRecording(raw);
      const fingerprint = createHash("sha256")
        .update(
          JSON.stringify(projected.targets.map((target: any) => [target.at_ms, target.fields.id])),
        )
        .digest("hex");
      if (fingerprints.has(fingerprint) || manifest.excluded_fingerprints?.includes(fingerprint))
        throw new InputExclusion("Duplicate mechanic sequence", { fingerprint });
      fingerprints.add(fingerprint);
      stage = "timeline-evaluation";
      const observations = projected.observations.map(({ raw_index, ...item }: any) => item);
      fs.writeFileSync(
        recording,
        JSON.stringify({ version: 1, end_ms: projected.end_ms, observations }),
      );
      const replay = JSON.parse(
        execFileSync(binary, ["compare", manifest.timeline, recording, "--json"], {
          encoding: "utf8",
        }),
      );
      assert.equal(replay.summaries[recording].status, "replayed");
      const rows = replay.recordings[recording];
      const diagnostics = rows.filter((row: any) =>
        ["ambiguous", "unresolved", "violation"].includes(row.action),
      );
      const phases = rows.filter((row: any) => row.action === "phase");
      const safeExit =
        phases.length === 2 &&
        phases[0].at_ms === 0 &&
        phases[0].detail === "hold/main" &&
        phases[1].at_ms === projected.exit_ms &&
        phases[1].detail === "castellation/main" &&
        !rows.some(
          (row: any) =>
            row.action === "event" &&
            ((row.at_ms >= projected.exit_ms && row.context.phase === "hold/main") ||
              (row.at_ms < projected.exit_ms && row.context.phase === "castellation/main")),
        );
      const targets = projected.targets.map((target: any) => {
        const candidates = rows.filter(
          (row: any) =>
            row.action === "event" && row.detail.includes(`: ${target.name} (visible=true,`),
        );
        const replayError = Math.min(
          ...candidates.map((row: any) => Math.abs(row.at_ms - target.at_ms)),
        );
        let queryError = 0;
        let queryDiagnostics: any[] = [];
        let safeQuery = true;
        if (target.lead_ms !== null) {
          const at = target.at_ms - target.lead_ms;
          const forecast = JSON.parse(
            execFileSync(
              binary,
              [
                "forecast",
                manifest.timeline,
                recording,
                "--at-ms",
                String(at),
                "--horizon-ms",
                String(manifest.horizon_ms),
              ],
              { encoding: "utf8" },
            ),
          );
          queryError = Math.min(
            ...forecast.events
              .filter((row: any) => row.name === target.name)
              .map((row: any) => Math.abs(row.at_ms - target.at_ms)),
          );
          queryDiagnostics = [...forecast.diagnostics, ...forecast.projection_diagnostics];
          safeQuery =
            !forecast.truncated &&
            forecast.phase === (at < projected.exit_ms ? "hold/main" : "castellation/main") &&
            forecast.status === "active" &&
            forecast.observations_used ===
              observations.filter((item: any) => item.at_ms <= at).length &&
            (at < projected.exit_ms ||
              forecast.events.every((row: any) => row.context.phase !== "hold/main"));
        }
        return {
          raw_index: target.raw_index,
          at_ms: target.at_ms,
          id: target.fields.id,
          name: target.name,
          stage: target.stage,
          lead_ms: target.lead_ms,
          replay_error_ms: Number.isFinite(replayError) ? replayError : null,
          query_error_ms:
            target.lead_ms === null || !Number.isFinite(queryError) ? null : queryError,
          diagnostics: queryDiagnostics,
          passed:
            safeQuery &&
            replayError <= manifest.tolerance_ms &&
            queryError <= manifest.tolerance_ms &&
            queryDiagnostics.length === 0,
        };
      });
      traces.push({
        status: "evaluated",
        input: item.input,
        sha256: item.sha256,
        trace: `${raw.report.code}/${raw.report.fights[0].id}`,
        fingerprint,
        origin_timestamp_ms: projected.origin_timestamp_ms,
        entry_event: projected.entry_event,
        exit_event: projected.exit_event,
        end_event: projected.end_event,
        exit_ms: projected.exit_ms,
        end_ms: projected.end_ms,
        observations: observations.length,
        hold_openings: projected.hold_openings,
        complete_bodies: projected.complete_bodies,
        observed_recurrences: projected.observed_recurrences,
        safe_exit: safeExit,
        diagnostics,
        targets,
        passed:
          safeExit && diagnostics.length === 0 && targets.every((target: any) => target.passed),
      });
    } catch (error) {
      traces.push({
        status: error instanceof InputExclusion ? "excluded" : "error",
        input: item.input,
        sha256: item.sha256,
        stage,
        error: String(error),
        ...(error instanceof InputExclusion ? { evidence: error.evidence } : {}),
        passed: error instanceof InputExclusion ? null : false,
      });
    }
  }
  const report = {
    manifest: manifestFile,
    manifest_sha256: hash(manifestFile),
    timeline_sha256: manifest.timeline_sha256,
    role: manifest.role,
    summary: holdoutSummary(traces),
    traces,
    not_checked: [
      "ACT signal equivalence and UI",
      "whole encounter",
      "loop recurrence beyond observed boundaries",
    ],
  };
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  console.log(JSON.stringify(report.summary));
  assert(report.summary.passed, `Evaluation failures saved: ${output}`);
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}

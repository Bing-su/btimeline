import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { varisP3Recording } from "./varis_p3_recording.ts";
import { holdoutSummary, InputExclusion } from "./types.ts";

const [binary, manifestFile, output] = process.argv.slice(2);
assert(
  binary && manifestFile && output,
  "Usage: bun scripts/verify_authored_varis_p3.ts BTIMELINE MANIFEST.json REPORT.json",
);
const hash = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
assert.equal(hash(manifest.timeline), manifest.timeline_sha256, "Candidate changed");
execFileSync(binary, ["validate", manifest.timeline]);
const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "varis-p3-evaluation-"));
const recording = path.join(temporary, "recording.json");
const fingerprints = new Set();
const traces = [];
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
      const projected = varisP3Recording(raw);
      const fingerprint = createHash("sha256")
        .update(
          JSON.stringify(
            projected.blocks.map((block) =>
              block.targets.map((target: any) => [target.at_ms - block.entry_ms, target.fields.id]),
            ),
          ),
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
      const transitions = rows
        .filter((row: any) => row.action === "phase")
        .map((row: any) => [row.at_ms, row.detail]);
      const safeEntry =
        JSON.stringify(transitions) ===
        JSON.stringify([
          [0, "ready"],
          ...projected.blocks.map((block) => [block.entry_ms, block.phase]),
        ]);
      const blocks = projected.blocks.map((block) => ({
        ...block,
        targets: block.targets.map((target: any) => {
          const candidates = rows.filter(
            (row: any) =>
              row.action === "event" &&
              row.at_ms >= block.entry_ms &&
              row.at_ms <= block.end_ms &&
              row.detail.includes(`: ${target.name} (visible=true,`),
          );
          const replayError = Math.min(
            ...candidates.map((row: any) => Math.abs(row.at_ms - target.at_ms)),
          );
          const forecast = JSON.parse(
            execFileSync(
              binary,
              [
                "forecast",
                manifest.timeline,
                recording,
                "--at-ms",
                String(target.at_ms - target.lead_ms),
                "--horizon-ms",
                String(manifest.horizon_ms),
              ],
              { encoding: "utf8" },
            ),
          );
          const error = Math.min(
            ...forecast.events
              .filter((row: any) => row.name === target.name)
              .map((row: any) => Math.abs(row.at_ms - target.at_ms)),
          );
          const queryDiagnostics = [...forecast.diagnostics, ...forecast.projection_diagnostics];
          return {
            raw_index: target.raw_index,
            at_ms: target.at_ms,
            id: target.fields.id,
            name: target.name,
            lead_ms: target.lead_ms,
            replay_error_ms: Number.isFinite(replayError) ? replayError : null,
            query_error_ms: Number.isFinite(error) ? error : null,
            diagnostics: queryDiagnostics,
            passed:
              forecast.phase === block.phase &&
              forecast.status === "active" &&
              error <= manifest.tolerance_ms &&
              replayError <= manifest.tolerance_ms &&
              queryDiagnostics.length === 0,
          };
        }),
      }));
      traces.push({
        status: "evaluated",
        input: item.input,
        sha256: item.sha256,
        trace: `${raw.report.code}/${raw.report.fights[0].id}`,
        fingerprint,
        origin_timestamp_ms: projected.origin_timestamp_ms,
        entry_event: projected.entry_event,
        join_event: projected.join_event,
        end_ms: projected.end_ms,
        order: projected.order,
        observations: observations.length,
        safe_entry: safeEntry,
        diagnostics,
        blocks,
        passed:
          safeEntry &&
          diagnostics.length === 0 &&
          blocks.every((block) => block.targets.every((target: any) => target.passed)),
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
      "ACT signal equivalence and NPC capitalization",
      "UI visibility",
      "P1/P2 or later full fight",
      "Spark/Torch positioning order",
    ],
  };
  fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
  console.log(JSON.stringify(report.summary));
  assert(report.summary.passed, `Evaluation failures saved: ${output}`);
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}

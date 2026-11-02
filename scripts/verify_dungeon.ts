import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { holdoutSummary, InputExclusion } from "./verify_griffin_holdout.ts";

type Target = { raw_index: number; at_ms: number; fields: { id: string }; name: string };
type Boss = {
  phase: string;
  entry_ms: number;
  end_ms: number;
  entry_event: number;
  completion_event: number;
  targets: Target[];
};
type Dungeon = { origin_timestamp_ms: number; observations: any[]; bosses: Boss[] };

// Keep encounter interpretation pure; both dungeon pilots use the same I/O, input-failure and forecast checks.
export function verifyDungeon(project: (raw: any) => Dungeon, notChecked: string[]) {
  const [binary, manifestFile, output] = process.argv.slice(2);
  assert(
    binary && manifestFile && output,
    `Usage: bun ${process.argv[1]} BTIMELINE MANIFEST.json REPORT.json`,
  );
  const hash = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  const manifest = JSON.parse(fs.readFileSync(manifestFile, "utf8"));
  assert.equal(hash(manifest.timeline), manifest.timeline_sha256, "Candidate changed");
  execFileSync(binary, ["validate", manifest.timeline]);
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "dungeon-evaluation-"));
  const recording = path.join(temporary, "recording.json");
  const traces = [];
  const fingerprints = new Set();
  const clean = (rows: any[]) =>
    rows.filter((row) => ["ambiguous", "unresolved", "violation"].includes(row.action));
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
        const projected = project(raw);
        const fingerprint = createHash("sha256")
          .update(
            JSON.stringify(
              projected.bosses.map((boss) =>
                boss.targets.map((target: any) => [target.at_ms - boss.entry_ms, target.fields.id]),
              ),
            ),
          )
          .digest("hex");
        if (fingerprints.has(fingerprint) || manifest.excluded_fingerprints?.includes(fingerprint))
          throw new InputExclusion("Duplicate mechanic sequence", { fingerprint });
        fingerprints.add(fingerprint);
        stage = "timeline-evaluation";
        const end_ms = projected.bosses.at(-1)!.end_ms;
        const observations = projected.observations
          .filter((item: any) => item.at_ms <= end_ms)
          .map(({ raw_index, ...item }: any) => item);
        fs.writeFileSync(recording, JSON.stringify({ version: 1, end_ms, observations }));
        const replay = JSON.parse(
          execFileSync(binary, ["compare", manifest.timeline, recording, "--json"], {
            encoding: "utf8",
          }),
        );
        assert.equal(replay.summaries[recording].status, "replayed");
        const rows = replay.recordings[recording];
        const diagnostics = clean(rows);
        const transitions = rows.filter((row: any) => row.action === "phase");
        const safeEntry =
          JSON.stringify(transitions.map((row: any) => [row.at_ms, row.detail])) ===
          JSON.stringify(projected.bosses.map((boss) => [boss.entry_ms, boss.phase]));
        const bosses = projected.bosses.map((boss) => {
          const targets = boss.targets.map((target: any) => {
            const name = target.name;
            const executed = rows.filter(
              (row: any) =>
                row.action === "event" &&
                row.at_ms >= boss.entry_ms &&
                row.at_ms <= boss.end_ms &&
                row.detail.includes(`: ${name} (visible=true,`),
            );
            const error = Math.min(
              ...executed.map((row: any) => Math.abs(row.at_ms - target.at_ms)),
            );
            let forecast = null;
            if (target.at_ms - boss.entry_ms >= manifest.forecast_lead_ms) {
              const result = JSON.parse(
                execFileSync(
                  binary,
                  [
                    "forecast",
                    manifest.timeline,
                    recording,
                    "--at-ms",
                    String(target.at_ms - manifest.forecast_lead_ms),
                    "--horizon-ms",
                    String(manifest.horizon_ms),
                  ],
                  { encoding: "utf8" },
                ),
              );
              const queryError = Math.min(
                ...result.events
                  .filter((row: any) => row.name === name)
                  .map((row: any) => Math.abs(row.at_ms - target.at_ms)),
              );
              forecast = {
                error_ms: Number.isFinite(queryError) ? queryError : null,
                passed:
                  result.phase === boss.phase &&
                  result.status === "active" &&
                  queryError <= manifest.tolerance_ms,
                diagnostics: [...result.diagnostics, ...result.projection_diagnostics],
              };
            }
            return {
              raw_index: target.raw_index,
              at_ms: target.at_ms,
              id: target.fields.id,
              error_ms: Number.isFinite(error) ? error : null,
              forecast,
              passed:
                error <= manifest.tolerance_ms &&
                (!forecast || (forecast.passed && forecast.diagnostics.length === 0)),
            };
          });
          return {
            phase: boss.phase,
            entry_ms: boss.entry_ms,
            end_ms: boss.end_ms,
            entry_event: boss.entry_event,
            completion_event: boss.completion_event,
            targets,
          };
        });
        traces.push({
          status: "evaluated",
          input: item.input,
          sha256: item.sha256,
          trace: `${raw.report.code}/${raw.report.fights[0].id}`,
          fingerprint,
          origin_timestamp_ms: projected.origin_timestamp_ms,
          cast_observations: observations.length,
          safe_entry: safeEntry,
          diagnostics,
          bosses,
          passed:
            safeEntry &&
            diagnostics.length === 0 &&
            bosses.every((boss) => boss.targets.every((target: any) => target.passed)),
        });
      } catch (error) {
        traces.push({
          status: error instanceof InputExclusion ? "excluded" : "error",
          input: item.input,
          sha256: item.sha256,
          stage,
          error: String(error),
          passed: error instanceof InputExclusion ? null : false,
          ...(error instanceof InputExclusion ? { evidence: error.evidence } : {}),
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
      not_checked: ["ACT equivalence and actual lifecycle signals", "UI visibility", ...notChecked],
    };
    fs.writeFileSync(output, JSON.stringify(report, null, 2) + "\n", { flag: "wx" });
    console.log(JSON.stringify(report.summary));
    assert(report.summary.passed, `Evaluation failures saved: ${output}`);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "griffin-inspection-test-"));
try {
  for (const name of ["tests", "timelines"])
    fs.symlinkSync(path.join(root, name), path.join(directory, name));
  const plan = JSON.parse(
    fs.readFileSync(path.join(root, "tests/fixtures/authored/griffin-holdout-plan.json"), "utf8"),
  );
  const [valid, broken] = plan.fights;
  const raw = path.join(directory, "raw");
  fs.mkdirSync(raw);
  fs.writeFileSync(
    path.join(raw, `${valid.report}_${valid.fight}.json`),
    JSON.stringify({
      report: {
        code: valid.report,
        fights: [{ ...valid, id: valid.fight }],
        masterData: { actors: [{ id: 1, name: "Thundergust Griffin" }] },
      },
      events: [
        { timestamp: 100, type: "cast", sourceID: 1, abilityGameID: 0xb0eb },
        { timestamp: 200, type: "damage", targetID: 1, targetResources: { hitPoints: 0 } },
      ],
    }),
  );
  fs.writeFileSync(path.join(raw, `${broken.report}_${broken.fight}.json`), "{}");
  const binary = path.join(directory, "btimeline");
  // A valid replay plus one failed inspector must fail overall; four absent files may still be excluded.
  fs.writeFileSync(
    binary,
    `#!/usr/bin/env bun
const [command, ...args] = process.argv.slice(2);
if (command === "inspect" && args[0].endsWith(${JSON.stringify(`${broken.report}_${broken.fight}.json`)})) {
  console.error("injected inspector failure");
  process.exit(42);
}
if (command === "compare") {
  const recording = args[1];
  console.log(JSON.stringify({ summaries: { [recording]: { status: "replayed" } }, recordings: { [recording]: [
    { action: "phase", at_ms: 0 },
    { action: "event", at_ms: 0, detail: "opening-thunderspark: Thunderspark (visible=true," }
  ] } }));
}
`,
    { mode: 0o755 },
  );
  const output = path.join(directory, "result.json");
  const result = Bun.spawnSync(
    ["bun", path.join(root, "scripts/verify_griffin_holdout.ts"), binary, raw, output],
    { cwd: directory },
  );
  assert.equal(result.exitCode, 1, result.stderr.toString());
  const report = JSON.parse(fs.readFileSync(output, "utf8"));
  assert.deepEqual(report.summary, {
    selected: 6,
    excluded: 4,
    eligible: 2,
    passed_traces: 1,
    passed: false,
  });
  const failure = report.traces.find(
    (trace: any) => trace.trace === `${broken.report}/${broken.fight}`,
  );
  assert.equal(failure.status, "error");
  assert.equal(failure.stage, "input-inspection");
  assert.match(failure.error, /injected inspector failure/);
  console.log("A failed inspector cannot be excluded to turn a partial evaluation into a pass");
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}

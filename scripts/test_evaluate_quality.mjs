// Check independent raw-cast accounting; optionally exercise the full runner with a local cactbot checkout.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { auditCoverage, auditReplayCoverage, metricsDelta } from "./evaluate_quality.mjs";

const directory = fs.mkdtempSync(path.join(os.tmpdir(), "btimeline-quality-test-"));
try {
  const fixture = JSON.parse(fs.readFileSync(new URL("../tests/fixtures/pull.json", import.meta.url), "utf8"));
  // Simultaneous instances share one display row, e.g. both opening casts remain represented.
  fixture.events.splice(1, 0, { ...fixture.events[0], sourceInstance: 2 });
  fixture.collection.eventCount = fixture.events.length;
  const file = path.join(directory, "train.json");
  fs.writeFileSync(file, JSON.stringify(fixture));
  const report = { mode: "raid", slots: [{}], input: { file }, emittedEventIndices: [0, 1, 2, 3] };
  assert.equal(auditCoverage(report).passed, true);
  report.emittedEventIndices = [0, 2, 3];
  assert.deepEqual(auditCoverage(report).files[0].unexplained, [1]);
  report.collapsedCasts = [{ representativeEventIndex: 0, omittedEventIndices: [1] }];
  const collapsed = auditCoverage(report);
  assert.equal(collapsed.passed, true);
  assert.equal(collapsed.totals.represented, 4);
  report.emittedEventIndices = [2, 3];
  assert.deepEqual(auditCoverage(report).files[0].unexplained, [0, 1], "Collapsed casts need an emitted representative");
  report.emittedEventIndices = [0, 2, 3];
  report.outputCoverage = [{ file, representedEventIndices: [0, 2, 3], excludedEventIndices: [1], omittedEventIndices: [] }];
  assert.equal(auditCoverage(report).totals.excluded, 1);
  report.outputCoverage[0].excludedEventIndices = [0];
  assert.throws(() => auditCoverage(report), /overlaps/);
  report.outputCoverage[0].excludedEventIndices = [1];
  report.mode = "alliance";
  report.bossSegments = [{ startMs: 0, endMs: 6000 }];
  assert.deepEqual(auditCoverage(report).files[0].outsideBossScope, [3]);
  assert(Math.abs(metricsDelta({ forecastRate: 0.8 }, { forecastRate: 0.6 }).forecastRate + 0.2) < 1e-12);
  const unseen = { pulls: [{ file: "holdout.json", unrepresentedEventIndices: [7], excludedEventIndices: [] }] };
  assert.equal(auditReplayCoverage(unseen).passed, false, "Holdout omissions cannot hide behind successful row replay");
  unseen.pulls[0].excludedFromGeneration = true;
  assert.equal(auditReplayCoverage(unseen).passed, true);
  assert.throws(() => auditReplayCoverage({ pulls: [{}] }), /Missing replay/);

  const consumer = process.argv[2];
  if (consumer) {
    const holdout = structuredClone(fixture);
    holdout.report.code = holdout.collection.reportCode = "independent";
    const second = path.join(directory, "holdout.json");
    fs.writeFileSync(second, JSON.stringify(holdout));
    const sha = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
    const manifest = path.join(directory, "inputs.csv");
    fs.writeFileSync(manifest, "file,sha256,report,fight,encounter,difficulty,candidate_group,split\n" +
      [[file, "synthetic", "train"], [second, "independent", "holdout"]].map(([file, report, split]) =>
        `${file},${sha(file)},${report},1,99199,123,${report},${split}\n`).join(""));
    const references = path.join(directory, "references.json");
    fs.writeFileSync(references, JSON.stringify({ "99199-123": path.resolve(consumer, "ui/raidboss/data/07-dt/raid/r11s.txt") }));
    const output = path.join(directory, "evaluation");
    const run = spawnSync(process.execPath, [new URL("evaluate_quality.mjs", import.meta.url).pathname, manifest, output,
      "--binary", path.resolve("target/debug/btimeline"), "--cactbot", path.resolve(consumer), "--references", references], { encoding: "utf8" });
    assert.equal(run.status, 1, `${run.stdout}\n${run.stderr}`);
    const result = JSON.parse(fs.readFileSync(path.join(output, "quality.json"), "utf8"));
    assert.equal(result.passed, false, "An unrelated reference's official trigger checks must fail");
    assert.equal(result.versions[0].groups.length, 2);
    for (const group of result.versions[0].groups) {
      assert.equal(group.status, "passed");
      assert.equal(group.coverage.passed, true);
      assert.equal(group.consumer.length, 2);
      assert(group.consumer.every((item) => item.passed), JSON.stringify(group));
      assert(group.official.failures.length > 0);
    }
  }
} finally {
  fs.rmSync(directory, { recursive: true, force: true });
}

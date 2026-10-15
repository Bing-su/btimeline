// Invoke cactbot's own tests in a temporary trigger context, preserving its source checkout.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { spawnSync } from "node:child_process";

const [input, checkout, reference, output] = process.argv.slice(2).map((value) => path.resolve(value));
assert(input && checkout && output, "Usage: verify_official_cactbot.mjs GENERATED.txt CACTBOT REFERENCE.txt REPORT.json");
assert(!fs.existsSync(output), "Official verification report already exists");
// Full type-aware emit removes upstream imports that share names with global values, e.g. Error.
if (process.env.BTIMELINE_OFFICIAL_WORKER !== "1") {
  const child = spawnSync(process.execPath, ["--loader", path.join(checkout, "node_modules/ts-node/esm.mjs"),
    path.resolve(process.argv[1]), input, checkout, reference, output], {
    stdio: "inherit", env: { ...process.env, BTIMELINE_OFFICIAL_WORKER: "1", TS_NODE_TRANSPILE_ONLY: "false",
      TS_NODE_PROJECT: path.join(checkout, "tsconfig.json"), TS_NODE_COMPILER_OPTIONS: JSON.stringify({ types: ["node", "mocha"] }) },
  });
  if (child.error) throw child.error;
  process.exit(child.status ?? 1);
}
const require = createRequire(path.join(checkout, "package.json"));
const Mocha = require("mocha");
const directory = fs.mkdtempSync(path.join(os.tmpdir(), "btimeline-official-"));
const previous = process.cwd();
try {
  const trigger = [".ts", ".js"].map((extension) => reference.replace(/\.txt$/, extension)).find((file) => fs.existsSync(file));
  assert(trigger, "Reference timeline needs its matching trigger file");
  fs.writeFileSync(path.join(directory, "package.json"), '{"type":"module"}');
  fs.copyFileSync(input, path.join(directory, "timeline.txt"));
  // Retain real translations and timeline triggers; the official tests must see their actual context.
  fs.writeFileSync(path.join(directory, "timeline.js"), `import original from ${JSON.stringify(pathToFileURL(trigger).href)};\nexport default {\n  ...original,\n  timelineFile: 'timeline.txt',\n};\n`);
  const failures = [];
  class CaptureReporter {
    constructor(runner) {
      runner.on("fail", (test, error) => failures.push({ test: test.fullTitle(), error: error.message }));
      runner.on("end", () => {
        fs.writeFileSync(output, `${JSON.stringify({ stats: runner.stats, failures,
          input, reference, scope: "Unmodified cactbot test/helper/test_timeline.ts with the reference trigger's translations and timeline triggers. No live ACT or browser validation." }, null, 2)}\n`, { flag: "wx" });
      });
    }
  }
  const mocha = new Mocha({ reporter: CaptureReporter });
  mocha.suite.emit("pre-require", globalThis, "btimeline", mocha);
  process.chdir(checkout);
  const { default: testTimelineFiles } = await import(pathToFileURL(path.join(checkout, "test/helper/test_timeline.ts")));
  testTimelineFiles([path.join(directory, "timeline.txt")]);
  const count = await new Promise((resolve) => mocha.run(resolve));
  console.log(JSON.stringify({ failures: count, report: output }));
  process.exitCode = count ? 1 : 0;
} finally {
  process.chdir(previous);
  fs.rmSync(directory, { recursive: true, force: true });
}

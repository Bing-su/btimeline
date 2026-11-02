import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "dungeon-candidate-race-"));
try {
  const timeline = path.join(temporary, "timeline.kdl");
  const input = path.join(temporary, "input.json");
  const manifest = path.join(temporary, "manifest.json");
  const output = path.join(temporary, "result.json");
  const binary = path.join(temporary, "fake-cli");
  const runner = path.join(temporary, "run.ts");
  const hash = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
  fs.writeFileSync(timeline, 'timeline 1 "Candidate" {}\n');
  fs.writeFileSync(input, JSON.stringify({ report: { code: "test", fights: [{ id: 1 }] } }));
  fs.writeFileSync(
    manifest,
    JSON.stringify({
      timeline,
      timeline_sha256: hash(timeline),
      inputs: [{ input, sha256: hash(input) }],
    }),
  );
  // Simulate an editor changing the KDL between validation and replay; the subprocess results themselves look clean.
  fs.writeFileSync(
    binary,
    `#!/usr/bin/env bun
import fs from "node:fs";
const [command,...args]=process.argv.slice(2);
if(command==="inspect")fs.appendFileSync(${JSON.stringify(timeline)},"// changed during evaluation\\n");
if(command==="compare")console.log(JSON.stringify({summaries:{[args[1]]:{status:"replayed"}},recordings:{[args[1]]:[{action:"phase",at_ms:0,detail:"boss"}]}}));
`,
  );
  fs.chmodSync(binary, 0o755);
  fs.writeFileSync(
    runner,
    `import { verifyDungeon } from ${JSON.stringify(path.join(import.meta.dir, "verify_dungeon.ts"))};
verifyDungeon(()=>({origin_timestamp_ms:0,observations:[],bosses:[{phase:"boss",entry_ms:0,end_ms:1000,entry_event:0,completion_event:0,targets:[]}]}),[]);`,
  );
  assert.throws(
    () => execFileSync(process.execPath, [runner, binary, manifest, output], { stdio: "pipe" }),
    (error: any) =>
      error.status !== 0 && error.stderr.toString().includes("Candidate changed during evaluation"),
  );
  assert(!fs.existsSync(output), "Never publish clean results for a changed candidate");
} finally {
  fs.rmSync(temporary, { recursive: true, force: true });
}
console.log("Dungeon evaluation rejects a candidate edited between subprocess calls");

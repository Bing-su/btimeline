// Verify converted drafts with the consumer itself, e.g. see docs/cactbot-e2e.md for invocation.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";

const [evaluation, cactbot, binary = "target/release/btimeline", ...supplemental] =
  process.argv.slice(2);
assert(evaluation && cactbot, "Usage: verify_cactbot.mjs EVALUATION CACTBOT [BINARY]");
const root = path.resolve(evaluation);
const consumer = path.resolve(cactbot);
const require = createRequire(path.join(consumer, "package.json"));
const { load: yaml } = require("js-yaml");
const imported = (file) => import(pathToFileURL(path.join(consumer, file)).href);
const { TimelineParser } = await imported("ui/raidboss/timeline_parser.ts");
const { TimelineController, TimelineUI } = await imported("ui/raidboss/timeline.ts");
const { default: defaults } = await imported("ui/raidboss/raidboss_options.ts");
const { default: definitions } = await imported("resources/netlog_defs.ts");
const read = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
const run = (...args) => execFileSync(path.resolve(binary), args, { encoding: "utf8" });
const hash = (file) =>
  require("node:crypto").createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const epoch = 1700000000000;
// Deliver the consumer's real start channel, e.g. SetInCombat(true) alone never starts its clock.
const combatStartLine = "260|2026-10-07T00:00:00.0000000+00:00|1|1|1|1|";
let now = epoch;
let timerId = 0;
const timers = new Map();

// Run the real engine's scheduled callbacks against a deterministic clock, e.g. window expiry.
globalThis.window = {
  setTimeout(callback, delay) {
    const id = ++timerId;
    timers.set(id, { callback, at: now + Math.max(1, delay) });
    return id;
  },
  clearTimeout(id) {
    timers.delete(id);
  },
};
Date.now = () => now;
function advance(target) {
  let updates = 0;
  while (timers.size) {
    const [id, timer] = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
    if (timer.at > target) break;
    assert(++updates < 100000, "Engine timer loop did not advance");
    timers.delete(id);
    now = timer.at;
    timer.callback();
  }
  now = target;
}

class CaptureUI extends TimelineUI {
  added = 0;
  visible = new Map();
  OnAddTimer(_time, event) {
    this.added++;
    this.visible.set(event.id, event);
  }
  OnRemoveTimer(event) {
    this.visible.delete(event.id);
  }
}

// Encode only available FFLogs fields into canonical network lines; this is not an ACT capture.
function signals(log, areaClear = false) {
  const actors = new Map(log.report.masterData.actors.map((a) => [a.id, a.name]));
  const abilities = new Map(log.report.masterData.abilities.map((a) => [a.gameID, a.name]));
  const start = log.report.fights[0].startTime;
  const result = log.events
    .flatMap((event, index) => {
      const type = { cast: "Ability", begincast: "StartsUsing" }[event.type];
      if (!type) return [];
      const def = definitions[type];
      const fields = Array.from({ length: Math.max(...Object.values(def.fields)) + 1 }, () => "0");
      const values = {
        type: def.type,
        timestamp: "2026-10-07T00:00:00.0000000+00:00",
        sourceId: "40000001",
        source: actors.get(event.sourceID) ?? "",
        id: event.abilityGameID?.toString(16).toUpperCase() ?? "",
        ability: abilities.get(event.abilityGameID) ?? "",
        targetId: "10000001",
        target: actors.get(event.targetID) ?? "",
        castTime: "1.000",
      };
      for (const [key, value] of Object.entries(values))
        if (key in def.fields) fields[def.fields[key]] = value;
      return [{ index, at: event.timestamp - start, line: `${fields.join("|")}|` }];
    })
    .sort((a, b) => a.at - b.at);
  if (areaClear) {
    // Model observed boss boundaries, e.g. clear boss 1 and restart combat before boss 2.
    const enemies = new Set(log.report.fights[0].enemyNPCs.map((actor) => actor.id));
    const bosses = log.report.masterData.actors.filter(
      (actor) => actor.subType === "Boss" && enemies.has(actor.id),
    );
    const spans = bosses
      .flatMap((actor) => {
        const times = log.events
          .filter((event) => event.sourceID === actor.id || event.targetID === actor.id)
          .map((event) => event.timestamp - start);
        return times.length ? [[Math.min(...times), Math.max(...times)]] : [];
      })
      .sort((a, b) => a[0] - b[0]);
    const merged = [];
    for (const span of spans) {
      const previous = merged.at(-1);
      if (previous && span[0] <= previous[1]) previous[1] = Math.max(previous[1], span[1]);
      else merged.push(span);
    }
    merged.slice(1).forEach((span, index) => {
      result.push({
        at: merged[index][1] + 1,
        lifecycle: "areaClear",
        line: "41|2026-10-07T00:00:00.0000000+00:00|0|7DE|0|0|0|",
      });
      result.push({ at: span[0], lifecycle: "combatStart", line: combatStartLine });
    });
    result.sort((a, b) => a.at - b.at || Number(!a.lifecycle) - Number(!b.lifecycle));
  }
  return result;
}

function playback(text, source, entryIndices, expected, forceStart = false) {
  now = epoch;
  timers.clear();
  const ui = new CaptureUI();
  const controller = new TimelineController({ ...defaults }, ui, { "draft.txt": text });
  controller.SetActiveTimeline(["draft.txt"], [], [], [], [], 0);
  const engine = ui.timeline;
  const byLine = new Map(
    [...engine.events]
      .sort((a, b) => a.lineNumber - b.lineNumber)
      .map((event, i) => [event.lineNumber, entryIndices[i]]),
  );
  const matches = [];
  let combatStarts = 0;
  let areaClears = 0;
  let signalIndex;
  const originalJump = engine.OnLogLineJump.bind(engine);
  // Observe selection without replacing it, e.g. repeated IDs must use the consumer's active windows.
  engine.OnLogLineJump = (sync, time) => {
    if (sync.origInput?.id === "7DE") {
      areaClears++;
      originalJump(sync, time);
      return;
    }
    if (sync.origInput?.inGameCombat === "1") {
      combatStarts++;
      originalJump(sync, time);
      return;
    }
    const match = {
      entryIndex: byLine.get(sync.lineNumber),
      eventIndex: signalIndex,
      jump: sync.jump,
    };
    if (sync.jump !== undefined) match.visibleBefore = [...ui.visible.values()].map((e) => e.name);
    matches.push(match);
    originalJump(sync, time);
    if (sync.jump !== undefined) match.visibleAfter = [...ui.visible.values()].map((e) => e.name);
  };
  const initialActiveSyncs = engine.activeNetSyncs.length;
  if (forceStart) engine.SyncTo(0, now);
  controller.SetInCombat(true);
  controller.OnNetLog({ rawLine: combatStartLine });
  for (const signal of source) {
    advance(epoch + signal.at);
    signalIndex = signal.index;
    controller.OnNetLog({ rawLine: signal.line });
    if (signal.lifecycle === "areaClear")
      assert.equal(engine.timebase, 0, "Area clear did not stop");
  }
  const observed = new Set(matches.map((m) => `${m.entryIndex}:${m.eventIndex}`));
  const result = {
    matches: matches.length,
    jumps: matches.filter((m) => m.jump !== undefined).length,
    timersAdded: ui.added,
    runningBeforeReset: engine.timebase !== 0,
    missingExpected: [...expected].filter((key) => !observed.has(key)).length,
    unexpected: [...observed].filter((key) => !expected.has(key)).length,
    initialActiveSyncs,
    jumpDetails: matches.filter((m) => m.jump !== undefined),
    combatStarts,
    areaClears,
    expectedCombatStarts: 1 + source.filter((signal) => signal.lifecycle === "combatStart").length,
    expectedAreaClears: source.filter((signal) => signal.lifecycle === "areaClear").length,
  };
  controller.SetInCombat(false);
  result.resetStopped = engine.timebase === 0 && ui.visible.size === 0 && timers.size === 0;
  assert(result.resetStopped, "Reset left an active clock/timer");
  // Reuse the same loaded engine after a wipe, e.g. a fresh 105 entry must reproduce its own matches.
  const reentryBase = epoch + 10000000;
  now = reentryBase;
  matches.length = 0;
  if (forceStart) engine.SyncTo(0, now);
  controller.SetInCombat(true);
  controller.OnNetLog({ rawLine: combatStartLine });
  for (const signal of source) {
    advance(reentryBase + signal.at);
    signalIndex = signal.index;
    controller.OnNetLog({ rawLine: signal.line });
  }
  const reentered = new Set(matches.map((m) => `${m.entryIndex}:${m.eventIndex}`));
  assert.deepEqual(reentered, observed, "Reset changed raw-signal matches");
  result.reentryRunning = engine.timebase !== 0;
  result.reentryCombatStarts = combatStarts - result.combatStarts;
  controller.SetInCombat(false);
  assert(engine.timebase === 0 && ui.visible.size === 0 && timers.size === 0);
  return result;
}

function verify(folder, splits) {
  const file = path.join(folder, "draft.yaml");
  run("validate", file);
  const output = path.join(folder, "draft.txt");
  run("convert", file, "-o", output);
  const text = fs.readFileSync(output, "utf8");
  const document = yaml(fs.readFileSync(file, "utf8"));
  // Conversion prepends lifecycle resets without FFLogs slots, e.g. alliance adds a second reset.
  const entryIndices = [
    ...(document.resetOn ?? ["wipe"]).map(() => undefined),
    ...document.entries.flatMap((e, index) => (e.kind === "event" ? [index] : [])),
  ];
  const parser = new TimelineParser(text, [], []);
  assert.equal(parser.events.length, entryIndices.length, "Conversion dropped events");
  const result = {
    folder,
    textSha256: hash(output),
    parserErrors: parser.errors,
    events: parser.events.length,
    syncs: parser.syncStarts.length,
    pulls: [],
  };
  for (const split of splits) {
    const replay = read(path.join(folder, `${split}.replay.json`));
    for (const pull of replay.pulls) {
      const source = signals(read(pull.file), (document.resetOn ?? ["wipe"]).includes("areaClear"));
      const expected = new Set(
        pull.replay.rows.flatMap((row) =>
          row.matches.map((m) => `${row.entryIndex}:${m.eventIndex}`),
        ),
      );
      const expectedRegexMatches = pull.replay.rows.reduce((total, row) => {
        const ordinal = entryIndices.indexOf(row.entryIndex);
        const event = [...parser.events].sort((a, b) => a.lineNumber - b.lineNumber)[ordinal];
        return (
          total +
          row.matches.filter((m) =>
            event?.sync?.regex.test(source.find((s) => s.index === m.eventIndex)?.line ?? ""),
          ).length
        );
      }, 0);
      const original = playback(text, source, entryIndices, expected);
      result.pulls.push({
        split,
        report: pull.report,
        fight: pull.fight,
        termination: pull.termination,
        expectedMatches: expected.size,
        expectedRegexMatches,
        original,
        unrepresentedCasts: pull.unrepresentedEventIndices.length,
        totalRawCasts: source.filter((s) => s.line.startsWith("21|")).length,
      });
    }
  }
  return result;
}

const frozen = read(path.join(root, "evaluation.json"));
assert.equal(hash(path.resolve(binary)), frozen.binarySha256, "Evaluation used a different binary");
// Compare the exact window edges with the internal inclusive policy, e.g. 10s +/- 2.5s.
const boundaryChecks = [7500, 10000, 12500].map((at) => ({
  atMs: at,
  consumer: playback(
    '10.0 "Boundary" Ability { id: "1", source: "Boss" }',
    [{ index: 0, at, line: "21|2026-10-07T00:00:00.0000000+00:00|40000001|Boss|1|Boundary|" }],
    [0],
    new Set(at < 12500 ? ["0:0"] : []),
    true,
  ),
}));
const results = frozen.groups.map((group) =>
  verify(
    path.join(root, group.evaluation, `${group.group.encounter}-${group.group.difficulty}`),
    group.holdout ? ["train", "holdout"] : ["train"],
  ),
);
for (const folder of supplemental) results.push(verify(path.resolve(folder), ["train"]));

// Exercise accepted P7/P8 paths absent from the real corpus, e.g. distinct clear and wipe exits.
function fixture(code, rows, encounter, kill, end = 45000) {
  end = Math.max(end, ...rows.map((row) => row[0] + 1000));
  return {
    report: {
      code,
      revision: 1,
      startTime: 0,
      endTime: 1000 + end,
      masterData: {
        lang: "en",
        gameVersion: 1,
        logVersion: 76,
        actors: [
          { id: 10, name: "Boss", gameID: 99901, type: "NPC", subType: "Boss" },
          { id: 11, name: "Helper", gameID: 99902, type: "NPC", subType: "NPC" },
          ...(rows.some((row) => row[1] === 12)
            ? [{ id: 12, name: "Second Boss", gameID: 99903, type: "NPC", subType: "Boss" }]
            : []),
        ],
        abilities: [...new Set(rows.map((r) => r[2]))].map((id) => ({
          gameID: id,
          name: `Ability ${id}`,
          type: "1",
        })),
      },
      fights: [
        {
          id: 2,
          name: `Unknown ${encounter}`,
          encounterID: encounter,
          difficulty: 9,
          startTime: 1000,
          endTime: 1000 + end,
          inProgress: false,
          kill,
          enemyNPCs: [
            { id: 10, gameID: 99901 },
            { id: 11, gameID: 99902 },
            ...(rows.some((row) => row[1] === 12) ? [{ id: 12, gameID: 99903 }] : []),
          ],
          enemyPets: [],
        },
      ],
    },
    collection: {
      schemaVersion: 1,
      toolVersion: "0.1.0",
      collectedAtUnixMs: 1,
      requests: {},
      complete: true,
      nextPageTimestamp: null,
      reportCode: code,
      fightID: 2,
      startTime: 1000,
      endTime: 1000 + end,
      eventCount: rows.length,
      pageCount: 1,
      pageStartTimes: [1000],
    },
    events: rows.map(([at, actor, ability]) => ({
      timestamp: 1000 + at,
      type: "cast",
      sourceID: actor,
      abilityGameID: ability,
      fight: 2,
    })),
  };
}
const branchA = [
  [1000, 10, 90001],
  [5000, 11, 90002],
  [6000, 11, 90007],
  [7000, 11, 90005],
  [9000, 10, 90004],
  [11000, 10, 90008],
];
const branchB = [
  [2000, 10, 90001],
  [12000, 11, 90003],
  [13000, 11, 90007],
  [14000, 11, 90006],
  [16000, 10, 90004],
  [18000, 10, 90008],
];
const cases = [
  { name: "p7-branch", encounter: 9999, rows: [branchA, branchB] },
  ...[1, 2].map((width) => {
    const rows = [[1000, 10, 91000]];
    for (let round = 0; round < 3; round++) {
      rows.push([5000 + round * 10000, 10, 91001]);
      if (width === 2) rows.push([8000 + round * 10000, 11, 91002]);
    }
    rows.push([38000, 10, 91003], [42000, 10, 91004]);
    return { name: `p8-repeat-${width}`, encounter: 99000 + width, rows: [rows, rows] };
  }),
  { name: "single-file", encounter: 9998, rows: [branchA] },
];
// Preserve accepted repeat jumps when rebasing bosses, e.g. a loop in boss 1 followed by boss 2.
const repeated = cases.find((item) => item.name === "p8-repeat-2");
cases.push({
  name: "p8-multi-boss",
  encounter: 9997,
  mode: "alliance",
  rows: repeated.rows.map((rows) => [...rows, [50000, 12, 91005]]),
});
for (const item of cases) {
  const folder = path.join(root, "synthetic", item.name);
  fs.mkdirSync(path.join(folder, "train"), { recursive: true });
  fs.mkdirSync(path.join(folder, "holdout"));
  for (const split of ["train", "holdout"])
    item.rows.forEach((rows, index) =>
      fs.writeFileSync(
        path.join(folder, split, `${index}.json`),
        JSON.stringify(
          fixture(`${item.name}-${split}-${index}`, rows, item.encounter, index === 0),
        ),
      ),
    );
  const input = path.join(folder, "train");
  const draft = path.join(folder, "draft.yaml");
  run(
    "generate",
    item.name === "single-file" ? path.join(input, "0.json") : input,
    "-o",
    draft,
    ...(item.mode ? ["--mode", item.mode] : []),
  );
  for (const split of ["train", "holdout"])
    run("replay", draft, path.join(folder, split), "-o", path.join(folder, `${split}.replay.json`));
  const generation = read(path.join(folder, "draft.report.json"));
  if (item.name.startsWith("p8")) assert.equal(generation.repeats.accepted, true);
  results.push(verify(folder, ["train", "holdout"]));
}

// Check the real HTML view too; jsdom verifies emitted DOM, not Chromium layout or live ACT delivery.
const disposeDom = require("jsdom-global")(
  '<style>.timer-bar { animation-name: none; }</style><div id="timeline-container"><div id="timeline"></div></div>',
  { pretendToBeVisual: true },
);
window.setTimeout = () => 1;
window.clearTimeout = () => {};
window.requestAnimationFrame = () => 0;
const { HTMLTimelineUI } = await imported("ui/raidboss/html_timeline_ui.ts");
const domChecks = results.map((group) => {
  const text = fs.readFileSync(path.join(group.folder, "draft.txt"), "utf8");
  const ui = new HTMLTimelineUI({ ...defaults });
  const controller = new TimelineController({ ...defaults }, ui, { "draft.txt": text });
  controller.SetActiveTimeline(["draft.txt"], [], [], [], [], 0);
  now = epoch;
  controller.SetInCombat(true);
  controller.OnNetLog({ rawLine: combatStartLine });
  const bars = [...document.querySelectorAll("#timeline timer-bar")].map((bar) => bar.lefttext);
  controller.SetInCombat(false);
  const remainingBars = document.querySelectorAll("#timeline timer-bar").length;
  assert.equal(remainingBars, 0, "HTML reset retained bars");
  // Re-enter the same loaded timeline after reset, e.g. a standalone 105 pull after a wipe.
  now = epoch + 10000000;
  controller.SetInCombat(true);
  controller.OnNetLog({ rawLine: combatStartLine });
  const reentryBars = [...document.querySelectorAll("#timeline timer-bar")].map(
    (bar) => bar.lefttext,
  );
  assert.deepEqual(reentryBars, bars, "Reset changed independent re-entry previews");
  controller.SetInCombat(false);
  assert.equal(document.querySelectorAll("#timeline timer-bar").length, 0);
  return { folder: group.folder, bars, remainingBars, reentryBars };
});
disposeDom();
const result = {
  date: "2026-10-07",
  btimelineCommit: execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
  binarySha256: frozen.binarySha256,
  manifestSha256: frozen.manifestSha256,
  cactbotCommit: execFileSync("git", ["-C", consumer, "rev-parse", "HEAD"], {
    encoding: "utf8",
  }).trim(),
  cactbotVersion: read(path.join(consumer, "package.json")).version,
  nodeVersion: process.version,
  results,
  domChecks,
  boundaryChecks,
  scope:
    "Actual cactbot parser/controller/engine, UI callbacks and HTML view under jsdom; reconstructed network lines, deterministic timers. InCombat at FFLogs fight start is modeled, not observed in FFLogs. No ACT capture, browser layout, locale replacement, or live game validation.",
};
fs.writeFileSync(path.join(root, "cactbot.json"), `${JSON.stringify(result, null, 2)}\n`);
const pulls = results.flatMap((r) => r.pulls);
console.log(
  JSON.stringify(
    {
      timelines: results.length,
      pulls: pulls.length,
      parserErrors: results.reduce((n, r) => n + r.parserErrors.length, 0),
      expectedMatches: pulls.reduce((n, p) => n + p.expectedMatches, 0),
      expectedRegexMatches: pulls.reduce((n, p) => n + p.expectedRegexMatches, 0),
      naturalStarts: pulls.filter((p) => p.original.runningBeforeReset).length,
      report: path.join(root, "cactbot.json"),
    },
    null,
    2,
  ),
);
// Fail the gate when the consumer cannot reproduce represented internal matches, e.g. anchored fields.
process.exitCode =
  boundaryChecks.some((b) => b.consumer.missingExpected || b.consumer.unexpected) ||
  results.some(
    (r) =>
      r.parserErrors.length ||
      r.pulls.some(
        (p) =>
          p.original.missingExpected ||
          p.original.unexpected ||
          !p.original.runningBeforeReset ||
          p.original.combatStarts !== p.original.expectedCombatStarts ||
          p.original.areaClears !== p.original.expectedAreaClears ||
          !p.original.reentryRunning ||
          p.original.reentryCombatStarts !== p.original.expectedCombatStarts,
      ),
  )
    ? 1
    : 0;

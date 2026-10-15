// Measure real consumer predictions, e.g. node --loader CACTBOT/node_modules/ts-node/esm.mjs scripts/verify_phase_schedule.ts out/fight.yaml CACTBOT.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import type {
  CollectedLog,
  ConsumerEvent,
  ConsumerTimeline,
  GenerationReport,
  ParsedTimeline,
  ReplayReport,
  TimelineDocument,
} from "./types.ts";

const [file, consumer, replayFile = file?.replace(/\.yaml$/, ".replay.json")] =
  process.argv.slice(2);
assert(file && consumer, "Usage: verify_phase_schedule.ts YAML CACTBOT [REPLAY]");
const root = path.resolve(consumer);
const require = createRequire(path.join(root, "package.json"));
const imported = (file: string) => import(pathToFileURL(path.join(root, file)).href);
const { TimelineParser } = await imported("ui/raidboss/timeline_parser.ts");
const { TimelineController, TimelineUI } = await imported("ui/raidboss/timeline.ts");
const { default: defaults } = await imported("ui/raidboss/raidboss_options.ts");
const { default: definitions } = await imported("resources/netlog_defs.ts");
const read = <T>(file: string): T => JSON.parse(fs.readFileSync(file, "utf8"));
const document: TimelineDocument = require("js-yaml").load(fs.readFileSync(file, "utf8"));
const text = fs.readFileSync(file.replace(/\.yaml$/, ".txt"), "utf8");
const generation = read<GenerationReport>(file.replace(/\.yaml$/, ".report.json"));
const replay = read<ReplayReport>(replayFile);
const epoch = 1700000000000;
let now = epoch;
let nextTimer = 0;
const timers = new Map<number, { callback: () => void; at: number }>();
// Only timer methods are needed by the consumer, e.g. advancing expiry without a browser.
globalThis.window = {
  setTimeout(callback: () => void, delay: number) {
    const id = ++nextTimer;
    timers.set(id, { callback, at: now + Math.max(1, delay) });
    return id;
  },
  clearTimeout(id: number) {
    timers.delete(id);
  },
} as unknown as Window & typeof globalThis;
Date.now = () => now;
function advance(target: number) {
  let steps = 0;
  while (timers.size) {
    const [id, timer] = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
    if (timer.at > target) break;
    assert(++steps < 100000, "Consumer timer did not advance");
    timers.delete(id);
    now = timer.at;
    timer.callback();
  }
  now = target;
}
class CaptureUI extends TimelineUI {
  declare timeline: ConsumerTimeline;
  visible = new Map<string, ConsumerEvent>();
  OnAddTimer(_time: number, event: ConsumerEvent) {
    this.visible.set(event.id, event);
  }
  OnRemoveTimer(event: ConsumerEvent) {
    this.visible.delete(event.id);
  }
}
const parser: ParsedTimeline = new TimelineParser(text, [], []);
assert.deepEqual(parser.errors, [], "Consumer parser rejected the timeline");
const indices = [
  ...(document.resetOn ?? ["wipe"]).map(() => undefined),
  ...document.entries.flatMap((entry, index) => (entry.kind === "event" ? [index] : [])),
];
assert.equal(parser.events.length, indices.length);
const slotEntries = document.entries.flatMap((entry, index) =>
  entry.kind === "event" && entry.sync?.log !== "InCombat" ? [index] : [],
);
type ForecastTarget = { entry: number; at: number; name: string };
type StreamItem = {
  at: number;
  index?: number;
  reset?: boolean;
  line?: string;
  probe?: ForecastTarget;
};
type ForecastCounts = {
  expectedMatches: number;
  forecastTargets: number;
  forecasted: number;
  exposures: number;
  calibrated: number;
};
const results: (ForecastCounts & {
  file: string;
  excludedFromGeneration: boolean;
  missing: string[];
  unexpected: string[];
  absent: ForecastTarget[];
})[] = [];
for (const pull of replay.pulls) {
  const log = read<CollectedLog>(pull.file);
  const start = log.report.fights[0].startTime;
  const actors = new Map<number, string>(
    log.report.masterData.actors.map((actor) => [actor.id, actor.name]),
  );
  const abilities = new Map<number, string>(
    log.report.masterData.abilities.map((ability) => [ability.gameID, ability.name]),
  );
  const resetIndices = new Set(
    generation.schedule?.segments
      .find((item) => item.file === pull.file)
      ?.spans.flatMap((span) => (span.reset === null ? [] : [span.reset])) ?? [],
  );
  // Holdouts have their own recorded reloads, e.g. a reset in an unseen report is absent from train segments.
  for (const preview of pull.replay.previews.filter((preview) => preview.moment === "combatEnd")) {
    const index = log.events.findIndex(
      (event) =>
        (event.type === "zonechange" ||
          (event.type === "instancesealupdate" && event.sealType === 3)) &&
        event.timestamp - start === preview.sourceMs,
    );
    assert(index >= 0, "Reset has no source arena opening or reload");
    resetIndices.add(index);
  }
  const source = log.events.flatMap<StreamItem>((event, index) => {
    if (resetIndices.has(index)) return [{ index, at: event.timestamp - start, reset: true }];
    const type = (
      { cast: "Ability", begincast: "StartsUsing" } as Record<string, string | undefined>
    )[event.type];
    if (!type) return [];
    const def: { type: string; fields: Record<string, number> } = definitions[type];
    const fields = Array.from({ length: Math.max(...Object.values(def.fields)) + 1 }, () => "0");
    const values = {
      type: def.type,
      timestamp: "2026-10-14T00:00:00.0000000+00:00",
      sourceId: "40000001",
      source: actors.get(event.sourceID!) ?? "",
      id: event.abilityGameID?.toString(16).toUpperCase() ?? "",
      ability: abilities.get(event.abilityGameID!) ?? "",
      targetId: "10000001",
      target: actors.get(event.targetID!) ?? "",
      castTime: "1.000",
    };
    for (const [key, value] of Object.entries(values))
      if (key in def.fields) fields[def.fields[key]] = value;
    return [{ index, at: event.timestamp - start, line: `${fields.join("|")}|` }];
  });
  const targets = generation.slots!.flatMap((slot, i) => {
    if (slot.selector || slot.predictive === false) return [];
    const row = pull.replay.rows.find((row) => row.entryIndex === slotEntries[i])!;
    const packets = row.packetEventIndices ?? row.expectedEventIndices;
    const times = packets.map((index) => log.events[index].timestamp - start).sort((a, b) => a - b);
    // One forecast per mechanic wave or burst, e.g. a three-hit duration bar is one upcoming mechanic.
    const waves = times.filter((at, index) => index === 0 || at - times[index - 1] > 250);
    const burstDuration = slot.burstOffsetsMs?.at(-1) ?? 0;
    let origin: number | undefined;
    return waves
      .filter((at) => {
        if (burstDuration && origin !== undefined && at - origin <= burstDuration + 2500)
          return false;
        origin = at;
        return true;
      })
      .map((at) => ({ entry: slotEntries[i], at, name: slot.name ?? row.name }));
  });
  // Five seconds before each observed mechanic is a forecast check, e.g. completing a sync is too late.
  const probes = targets
    .filter((target) => target.at >= 5000)
    .map((target) => ({ at: target.at - 5000, probe: target }));
  const stream: StreamItem[] = [...source, ...probes].sort(
    (a, b) => a.at - b.at || Number(Boolean(a.probe)) - Number(Boolean(b.probe)),
  );
  now = epoch;
  timers.clear();
  const ui = new CaptureUI();
  const controller = new TimelineController({ ...defaults }, ui, { "draft.txt": text });
  controller.SetActiveTimeline(["draft.txt"], [], [], [], [], 0);
  const engine = ui.timeline;
  const byLine = new Map(
    [...engine.events]
      .sort((a, b) => a.lineNumber - b.lineNumber)
      .map((event, i) => [event.lineNumber, indices[i]]),
  );
  let signalIndex: number | undefined;
  const observed = new Set<string>();
  const originalJump = engine.OnLogLineJump.bind(engine);
  engine.OnLogLineJump = (sync, time) => {
    if (sync.origInput?.inGameCombat !== "1")
      observed.add(`${byLine.get(sync.lineNumber)}:${signalIndex}`);
    originalJump(sync, time);
  };
  controller.SetInCombat(true);
  controller.OnNetLog({ rawLine: "260|2026-10-14T00:00:00.0000000+00:00|1|1|1|1|" });
  let forecasted = 0;
  let exposures = 0;
  let calibrated = 0;
  const absent = [];
  for (const item of stream) {
    advance(epoch + item.at);
    if (item.reset) {
      controller.SetInCombat(false);
      continue;
    }
    if (item.probe) {
      const bars = [...ui.visible.values()].filter(
        (event) => engine.timebase + event.time * 1000 > now,
      );
      assert(
        bars.every((event) => !["--reference--", "--sync--", "--Reset--"].includes(event.name)),
        "Hidden trace evidence reached the UI",
      );
      if (bars.some((event) => byLine.get(event.lineNumber) === item.probe!.entry)) forecasted++;
      else if (absent.length < 10) absent.push(item.probe);
      for (const event of bars) {
        exposures++;
        const predicted = engine.timebase + event.time * 1000 - epoch;
        if (
          targets.some(
            (target) =>
              target.entry === byLine.get(event.lineNumber) &&
              Math.abs(target.at - predicted) <= 2500,
          )
        )
          calibrated++;
      }
    } else {
      signalIndex = item.index;
      controller.OnNetLog({ rawLine: item.line });
    }
  }
  const expected = new Set(
    pull.replay.rows.flatMap((row) =>
      row.matches.map((match) => `${row.entryIndex}:${match.eventIndex}`),
    ),
  );
  // The consumer may sync on every target packet, e.g. the internal clock audit spends the row after one packet.
  const allowed = new Set([
    ...expected,
    ...pull.replay.rows.flatMap((row) =>
      (row.packetEventIndices ?? []).map((index) => `${row.entryIndex}:${index}`),
    ),
    ...generation.slots!.flatMap((slot, i) =>
      (slot.samples ?? [])
        .filter((sample) => sample.file === pull.file)
        .flatMap((sample) => sample.eventIndices.map((index) => `${slotEntries[i]}:${index}`)),
    ),
  ]);
  const missing = [...expected].filter((key) => !observed.has(key));
  const unexpected = [...observed].filter((key) => !allowed.has(key));
  controller.SetInCombat(false);
  assert.equal(engine.timebase, 0);
  assert.equal(ui.visible.size, 0);
  assert.equal(timers.size, 0);
  results.push({
    file: pull.file,
    excludedFromGeneration: Boolean(pull.excludedFromGeneration),
    expectedMatches: expected.size,
    missing,
    unexpected,
    forecastTargets: probes.length,
    forecasted,
    exposures,
    calibrated,
    absent,
  });
}
const sum = (field: keyof ForecastCounts) =>
  results.reduce((total, result) => total + result[field], 0);
const summary = {
  pulls: results.length,
  expectedMatches: sum("expectedMatches"),
  forecastTargets: sum("forecastTargets"),
  forecasted: sum("forecasted"),
  exposures: sum("exposures"),
  calibrated: sum("calibrated"),
  missingMatches: results.reduce((n, result) => n + result.missing.length, 0),
  unexpectedMatches: results.reduce((n, result) => n + result.unexpected.length, 0),
};
// Match replay's validation scope while exposing every diagnostic failure, e.g. a wipe-only path is not a clear forecast.
const checked = results.filter((result) => !result.excludedFromGeneration);
const validation = {
  checkedPulls: checked.length,
  diagnosticPulls: results.length - checked.length,
  missingMatches: checked.reduce((n, result) => n + result.missing.length, 0),
  unexpectedMatches: checked.reduce((n, result) => n + result.unexpected.length, 0),
  allInputsPassed: summary.missingMatches === 0 && summary.unexpectedMatches === 0,
};
const output = replayFile.replace(/\.replay\.json$/, ".cactbot.json");
fs.writeFileSync(
  output,
  `${JSON.stringify({ summary, validation, results, generationFile: file, replayFile, scope: "Actual cactbot parser/controller/engine and timer callbacks; default six bars, 30s horizon, 5s forecast probes, 2.5s timing tolerance. Excluded inputs are diagnostic; summary includes their failures, validation checks non-excluded inputs. Network lines and reset lifecycle reconstructed from FFLogs; no live ACT capture." }, null, 2)}\n`,
);
console.log(JSON.stringify({ ...summary, validation, output }, null, 2));
assert.equal(validation.missingMatches, 0, "Consumer missed internal sync matches");
assert.equal(validation.unexpectedMatches, 0, "Consumer activated unexpected syncs");

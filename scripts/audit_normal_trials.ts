// Validation fixtures only: encounter and game-zone IDs jointly distinguish normal from extreme.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import {
  completeCycles,
  representedNodes,
  type Observation,
  type Pattern,
} from "./audit_dungeon_loops.ts";
import { compareFiles } from "./compare_cactbot.ts";

export const trials = {
  doomtrain: {
    encounter: 1547,
    zone: 1307,
    difficulty: 100,
    extremeEncounter: 1083,
    extremeZone: 1308,
  },
  enuo: { encounter: 1548, zone: 1361, difficulty: 100, extremeEncounter: 1084, extremeZone: 1362 },
} as const;
export type Trial = keyof typeof trials;

export function assertNormalFight(
  fight: { encounterID: number; difficulty: number; gameZone?: { id: number } },
  trial: Trial,
) {
  const expected = trials[trial];
  assert.equal(
    fight.encounterID,
    expected.encounter,
    "Wrong normal encounter; extreme must stay separate",
  );
  assert.equal(fight.difficulty, expected.difficulty, "Wrong difficulty");
  assert.equal(
    fight.gameZone?.id,
    expected.zone,
    "Wrong or missing game zone; difficulty=100 alone is insufficient",
  );
}

// Same-ID runs cannot identify a particular reference rotation, e.g. five Nothingness displays.
export function symmetricBody(body: Pattern[]): boolean {
  const keys = body.map((row) =>
    JSON.stringify([row.kind ?? "cast", [...row.ids].sort(), [...row.sources].sort()]),
  );
  return keys.some(
    (_, offset) =>
      offset > 0 && keys.every((key, index) => key === keys[(index + offset) % keys.length]),
  );
}

type Row = Awaited<ReturnType<typeof compareFiles>>["reference"]["rows"][number];
function pattern(row: Row): Pattern {
  const source = (row.evidence?.fields as Record<string, unknown> | undefined)?.source;
  return {
    ids: row.ids,
    sources: (Array.isArray(source) ? source : source === undefined ? [] : [source]) as string[],
    time: row.time,
    name: row.name,
    kind: row.evidence?.type === "StartsUsing" ? "begincast" : "cast",
  };
}
const observable = (row: Row) =>
  row.ids.length && ["Ability", "StartsUsing"].includes(row.evidence?.type ?? "");

export async function audit(trial: Trial, evidenceFile: string, cactbot: string) {
  const evidence = JSON.parse(await fs.readFile(evidenceFile, "utf8"));
  const kdl = evidenceFile.replace(/\.evidence\.json$/, ".kdl");
  assert.notEqual(kdl, evidenceFile, "Expected .evidence.json");
  const timeline = await fs.readFile(kdl, "utf8");
  assert.equal(
    new Bun.CryptoHasher("sha256").update(timeline).digest("hex"),
    evidence.timeline_sha256,
  );
  const referenceFile = path.join(cactbot, "ui/raidboss/data/07-dt/trial", trial + ".txt");
  const parsed = await compareFiles(referenceFile, referenceFile, cactbot);
  const backward = parsed.reference.forceJumps.filter(
    (jump) => jump.jump !== undefined && jump.jump < jump.time,
  );
  const loops = backward.map((jump) => {
    const rows = parsed.reference.rows.filter(
      (row) => row.time >= jump.jump! && row.time < jump.time && observable(row),
    );
    const active = rows.filter((row) => !row.evidence?.disabled);
    const body = (active.length ? active : rows).map(pattern);
    const closing = parsed.reference.rows.find(
      (row) => row.time === jump.time && observable(row) && !row.evidence?.disabled,
    );
    return {
      start: jump.jump!,
      end: jump.time,
      nominal_period_seconds: Number((jump.time - jump.jump!).toFixed(1)),
      projection: active.length ? "active signals" : "commented body annotations",
      body,
      closing: closing ? pattern(closing) : null,
      ambiguous_rotation: symmetricBody(body),
      traces: [] as any[],
    };
  });
  const inputs = [];
  for (const input of evidence.analysis.inputs) {
    const bytes = await fs.readFile(input.file);
    assert.equal(
      new Bun.CryptoHasher("sha256").update(bytes).digest("hex"),
      input.sha256,
      "Changed input: " + input.file,
    );
    const log = JSON.parse(bytes.toString("utf8"));
    assert.equal(log.report.fights.length, 1);
    const fight = log.report.fights[0];
    assertNormalFight(fight, trial);
    const actors = new Map<number, any>(
      log.report.masterData.actors.map((actor: any) => [actor.id, actor]),
    );
    const graph = evidence.progression.traces.find((trace: any) => trace.trace === input.trace);
    const represented = representedNodes(evidence, input.trace);
    const emitted = new Set<number>();
    for (const node of graph.nodes.filter((node: any) => represented.has(node.index))) {
      for (const index of input.observations[node.observation].indices) {
        const event = log.events[index];
        if (
          event.type === "cast" &&
          event.abilityGameID === node.activity.identifier.Ability &&
          actors.get(event.sourceID)?.name === node.activity.source
        )
          emitted.add(index);
      }
    }
    const samples: Observation[] = log.events
      .flatMap((event: any, index: number) => {
        if (!["cast", "begincast"].includes(event.type) || !Number.isInteger(event.abilityGameID))
          return [];
        return [
          {
            at: event.timestamp - fight.startTime,
            id: event.abilityGameID.toString(16).toUpperCase(),
            source: actors.get(event.sourceID)?.name?.trim()
              ? actors.get(event.sourceID)!.name
              : null,
            node: index,
            kind: event.type,
            entities: [String(event.sourceID) + "/" + String(event.sourceInstance ?? 1)],
          },
        ];
      })
      .sort((a: Observation, b: Observation) => a.at - b.at || a.node - b.node);
    inputs.push({
      file: input.file,
      trace: input.trace,
      sha256: input.sha256,
      encounter: fight.encounterID,
      difficulty: fight.difficulty,
      game_zone: fight.gameZone,
      kill: fight.kill,
      duration_ms: fight.endTime - fight.startTime,
    });
    for (const loop of loops) {
      if (!loop.body.length || !loop.closing) {
        loop.traces.push({ trace: input.trace, status: "unobservable boundary" });
        continue;
      }
      const windows = completeCycles(samples, loop.body, loop.closing);
      loop.traces.push({
        trace: input.trace,
        status: windows.length ? "ordered signal windows observed" : "no exact full window",
        windows: windows.map((window) => ({
          ...window,
          first_signal_offset_ms: Math.round((loop.body[0].time - loop.start) * 1000),
          reference_offset_adjusted_span_ms:
            window.observed_span_ms + Math.round((loop.body[0].time - loop.start) * 1000),
          emitted_members: window.body_nodes.filter((node) => emitted.has(node)).length,
        })),
        observed_pattern_counts: loop.body.map(
          (row) =>
            samples.filter(
              (sample) =>
                sample.kind === row.kind &&
                row.ids.includes(sample.id) &&
                (!row.sources.length ||
                  (sample.source !== null && row.sources.includes(sample.source))),
            ).length,
        ),
      });
    }
  }
  return {
    trial,
    identity: trials[trial],
    cactbotCommit: parsed.cactbotCommit,
    referenceFile,
    reference_sha256: new Bun.CryptoHasher("sha256")
      .update(await fs.readFile(referenceFile))
      .digest("hex"),
    evidenceFile,
    mode: evidence.mode,
    generated_loops: [...timeline.matchAll(/^\s*loop\s/gm)].length,
    inputs,
    loops,
    unobservable_controls: parsed.reference.rows
      .filter(
        (row) =>
          row.evidence &&
          !row.evidence.disabled &&
          !["Ability", "StartsUsing"].includes(row.evidence.type),
      )
      .map((row) => ({
        time: row.time,
        name: row.name,
        type: row.evidence!.type,
        fields: row.evidence!.fields,
      })),
    scope:
      "Exact ordered ID/source/type windows over raw casts and cast starts. Disjoint entities may share a 100ms wave; original event indices remain. Commented annotations are not runtime syncs. Symmetric bodies cannot identify add/beacon phase or unique rotations. No inferred MapEffect/NameToggle, final-KDL timing proof, or ACT execution.",
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: { output: { type: "string", short: "o" } },
  });
  const [trial, evidence, cactbot] = positionals;
  assert(
    trial in trials && evidence && cactbot && values.output,
    "Usage: audit_normal_trials.ts doomtrain|enuo EVIDENCE.json CACTBOT -o REPORT.json",
  );
  const report = await audit(trial as Trial, evidence, cactbot);
  await fs.writeFile(values.output, JSON.stringify(report, null, 2) + "\n");
  console.log(
    JSON.stringify({
      trial,
      inputs: report.inputs.length,
      generatedLoops: report.generated_loops,
      loops: report.loops.map((loop) => ({
        start: loop.start,
        period: loop.nominal_period_seconds,
        boundary: loop.closing?.kind,
        ambiguous: loop.ambiguous_rotation,
        tracesWithWindows: loop.traces.filter((trace) => trace.windows?.length).length,
      })),
    }),
  );
}

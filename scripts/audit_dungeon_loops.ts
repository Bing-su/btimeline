// Verify complete reference cycles, e.g. a repeated opener alone must not count as a loop.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { compareFiles } from "./compare_cactbot.ts";

export type Pattern = {
  ids: string[];
  sources: string[];
  time: number;
  name: string;
  kind?: "cast" | "begincast";
};
export type Observation = {
  id: string;
  source: string | null;
  at: number;
  node: number;
  kind?: "cast" | "begincast";
  entities?: string[];
};

// Bind rendered definitions to raw nodes; repeated iterations are not additional finite definitions.
export function representedNodes(evidence: any, trace: string): Set<number> {
  const finite = evidence.events.flatMap((event: any) =>
    Object.entries(event.observation_nodes ?? {})
      .filter(([key]) => key === trace)
      .map(([, node]) => node as number),
  );
  const repeated = (evidence.loops ?? []).flatMap((loop: any) =>
    (loop.events ?? []).flatMap((event: any) =>
      event.observations
        .filter((observation: any) => observation.trace === trace)
        .flatMap((observation: any) => observation.observation_nodes),
    ),
  );
  return new Set([...finite, ...repeated]);
}

const matches = (pattern: Pattern, sample: Observation) =>
  (pattern.kind ?? "cast") === (sample.kind ?? "cast") &&
  pattern.ids.includes(sample.id) &&
  (!pattern.sources.length || (sample.source !== null && pattern.sources.includes(sample.source)));

export function completeCycles(samples: Observation[], body: Pattern[], closing: Pattern) {
  assert(body.length > 0, "Missing observable loop body");
  assert(
    samples.every(
      (sample, index) =>
        Number.isSafeInteger(sample.at) &&
        sample.at >= 0 &&
        (index === 0 || samples[index - 1].at <= sample.at),
    ),
    "Observations must be ordered nonnegative integer milliseconds",
  );
  const patterns = [...body, closing];
  const relevant = samples.filter((sample) => patterns.some((pattern) => matches(pattern, sample)));
  const waves: Observation[][] = [];
  for (const sample of relevant) {
    const previous = waves.at(-1);
    // Reference-declared alternative IDs can describe one 100ms wave; retain every raw node.
    if (
      previous &&
      sample.at - previous[0].at <= 100 &&
      (previous.some((row) => row.id !== sample.id) ||
        (sample.entities?.length &&
          previous.every(
            (row) =>
              row.entities?.length &&
              row.entities.every((entity) => !sample.entities!.includes(entity)),
          ))) &&
      patterns.some(
        (pattern) => matches(pattern, sample) && previous.every((row) => matches(pattern, row)),
      )
    ) {
      previous.push(sample);
    } else waves.push([sample]);
  }
  const cycles = [];
  for (let start = 0; start + body.length < waves.length; start++) {
    const rows = waves.slice(start, start + patterns.length);
    if (!patterns.every((pattern, index) => rows[index].every((row) => matches(pattern, row))))
      continue;
    if (rows.at(-1)![0].at <= rows[0][0].at) continue;
    const origin = rows[0][0].at;
    cycles.push({
      start_ms: origin,
      end_ms: rows.at(-1)![0].at,
      observed_span_ms: rows.at(-1)![0].at - origin,
      nodes: rows.flatMap((wave) => wave.map((row) => row.node)),
      body_nodes: rows.slice(0, -1).flatMap((wave) => wave.map((row) => row.node)),
      // Timing is diagnostic; it does not resolve a structural mismatch or remove an observed cycle.
      max_offset_error_ms: Math.max(
        ...rows.map((row, index) =>
          Math.abs(
            row[0].at -
              origin -
              (Math.round(patterns[index].time * 1000) - Math.round(body[0].time * 1000)),
          ),
        ),
      ),
    });
  }
  return cycles;
}

type Row = Awaited<ReturnType<typeof compareFiles>>["reference"]["rows"][number];
function pattern(row: Row): Pattern {
  const source = (row.evidence?.fields as Record<string, unknown> | undefined)?.source;
  return {
    ids: row.ids,
    sources: (Array.isArray(source) ? source : source === undefined ? [] : [source]) as string[],
    time: row.time,
    name: row.name,
  };
}

export async function audit(evidenceFile: string, referenceFile: string, cactbot: string) {
  const evidence = JSON.parse(await fs.readFile(evidenceFile, "utf8"));
  // Reuse the official-parser adapter; commented syncs never become observed reference steps.
  const parsed = await compareFiles(referenceFile, referenceFile, cactbot);
  const reference = parsed.reference;
  const loops = reference.forceJumps.filter(
    (jump) => jump.jump !== undefined && jump.jump < jump.time,
  );
  const results = [];
  for (const loop of loops) {
    const body = reference.rows
      .filter(
        (row) =>
          row.time >= loop.jump! &&
          row.time < loop.time &&
          row.evidence?.type === "Ability" &&
          !row.evidence.disabled,
      )
      .map(pattern);
    const closingRow = reference.rows.find(
      (row) => row.time === loop.time && row.evidence?.type === "Ability" && !row.evidence.disabled,
    );
    if (!body.length || !closingRow) {
      results.push({ loop, status: "requires-non-Ability-boundary" });
      continue;
    }
    const seal = reference.rows
      .filter((row) => row.time <= loop.jump! && row.evidence?.type === "SystemLogMessage")
      .at(-1);
    const phaseRows = reference.rows.filter(
      (row) => row.time >= (seal?.time ?? 0) && row.time < loop.time,
    );
    const bosses = [
      ...new Set(
        phaseRows
          .flatMap((row) => pattern(row).sources)
          .filter((source) => evidence.analysis.boss_names.includes(source)),
      ),
    ];
    const traces = [];
    for (const input of evidence.analysis.inputs) {
      const graph = evidence.progression.traces.find(
        (trace: { trace: string }) => trace.trace === input.trace,
      );
      const ranges = evidence.alignment_diagnostics
        .filter(
          (row: { method: string; trace: string; source: string }) =>
            row.method === "boss-attempt-scope-v1" &&
            row.trace === input.trace &&
            bosses.includes(row.source),
        )
        .flatMap(
          (row: {
            source: string;
            attempts: {
              actor: number;
              instance: number;
              start_ms: number;
              end_ms: number | null;
              last_cast_ms: number;
            }[];
          }) =>
            row.attempts.map((attempt) => ({
              source: row.source,
              actor: attempt.actor,
              instance: attempt.instance,
              start: attempt.start_ms,
              end: attempt.end_ms ?? attempt.last_cast_ms,
              completionObserved: attempt.end_ms !== null,
            })),
        );
      if (!ranges.length) {
        traces.push({ trace: input.trace, status: "no-observed-entity-span" });
        continue;
      }
      // Failed attempts can contain a whole loop; match each observed span separately, never across resets.
      const unique = new Map<
        string,
        ReturnType<typeof completeCycles>[number] & { span: (typeof ranges)[number] }
      >();
      for (const range of ranges) {
        const samples: Observation[] = graph.nodes
          .filter(
            (node: { observation: number }) =>
              range.start <= input.observations[node.observation].at_ms &&
              input.observations[node.observation].at_ms <= range.end,
          )
          .map(
            (node: {
              index: number;
              observation: number;
              activity: { identifier: { Ability: number }; source: string | null };
            }) => ({
              node: node.index,
              at: input.observations[node.observation].at_ms,
              id: node.activity.identifier.Ability.toString(16).toUpperCase(),
              source: node.activity.source,
            }),
          );
        for (const cycle of completeCycles(samples, body, pattern(closingRow)))
          unique.set(JSON.stringify(cycle.nodes), { ...cycle, span: range });
      }
      const cycles = [...unique.values()];
      const represented = representedNodes(evidence, input.trace);
      const firstAbilityOffset = Math.round(body[0].time * 1000) - Math.round(loop.jump! * 1000);
      const coverage = cycles.map((cycle) => ({
        ...cycle,
        first_ability_offset_ms: firstAbilityOffset,
        period_ms: cycle.observed_span_ms + firstAbilityOffset,
        emitted_samples: cycle.body_nodes.filter((index) => represented.has(index)).length,
      }));
      traces.push({
        trace: input.trace,
        status: cycles.length
          ? "complete-reference-body-observed"
          : "no-exact-complete-reference-body",
        cycles: coverage,
      });
    }
    results.push({
      start: loop.jump,
      end: loop.time,
      nominal_period_seconds: Number((loop.time - loop.jump!).toFixed(1)),
      bosses,
      observable_steps: body.length,
      body,
      traces,
    });
  }
  const kdl = await fs.readFile(evidenceFile.replace(/\.evidence\.json$/, ".kdl"), "utf8");
  return {
    cactbotCommit: parsed.cactbotCommit,
    evidenceFile,
    referenceFile,
    generatedLoops: [...kdl.matchAll(/^\s*loop\s/gm)].length,
    loops: results,
    scope:
      "Exact ordered active Ability ID/source projection within individual observed boss entity spans, including incomplete attempts. Reference-declared alternative IDs may form one 100ms wave; all raw nodes are retained. Missing cycles may reflect short recordings, order uncertainty, alternative paths or unscoped entities. Not a loop learner, infinite-execution proof, StartsUsing/commented-step coverage, or ACT runtime validation.",
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: { output: { type: "string", short: "o" } },
  });
  assert(
    positionals.length === 3 && values.output,
    "Usage: audit_dungeon_loops.ts EVIDENCE.json REFERENCE.txt CACTBOT -o REPORT.json",
  );
  const report = await audit(positionals[0], positionals[1], positionals[2]);
  await fs.writeFile(values.output, JSON.stringify(report, null, 2) + "\n");
  console.log(
    JSON.stringify({
      generatedLoops: report.generatedLoops,
      loops: report.loops.map((loop) =>
        "bosses" in loop
          ? {
              bosses: loop.bosses,
              period: loop.nominal_period_seconds,
              traces: loop.traces.filter(
                (trace) => trace.status === "complete-reference-body-observed",
              ).length,
            }
          : loop,
      ),
    }),
  );
}

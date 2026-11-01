// Compare mechanics parsed by cactbot, e.g. repeated abilities and alternate IDs.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
import { parseArgs } from "node:util";
import type { ParsedTimeline } from "./types.ts";

type MechanicRow = {
  name: string;
  ids: string[];
  time: number;
  duration?: number;
  sync?: unknown;
  evidence?: { type: string; disabled?: boolean } | null;
};

export function describeUnmatched(rows: MechanicRow[]) {
  const result = {
    noNetworkEvidence: 0,
    activeEvidence: 0,
    commentedEvidence: 0,
    withoutIds: 0,
    byType: {} as Record<string, number>,
  };
  for (const row of rows) {
    if (!row.ids.length) result.withoutIds++;
    if (!row.evidence) {
      result.noNetworkEvidence++;
      continue;
    }
    if (row.evidence.disabled) result.commentedEvidence++;
    else result.activeEvidence++;
    result.byType[row.evidence.type] = (result.byType[row.evidence.type] ?? 0) + 1;
  }
  return result;
}

// Keep simultaneous matches visible for source review, e.g. multiple Explosion observations.
export function simultaneousMatches<T extends MechanicRow>(
  matches: { reference: T; generated: T }[],
) {
  const groups = new Map<
    string,
    { time: number; id: string; matches: { reference: T; generated: T }[] }
  >();
  for (const match of matches) {
    for (const id of new Set(match.generated.ids)) {
      const key = JSON.stringify([match.generated.time, id]);
      const group = groups.get(key) ?? { time: match.generated.time, id, matches: [] };
      group.matches.push(match);
      groups.set(key, group);
    }
  }
  return [...groups.values()].filter((group) => group.matches.length > 1);
}

// Preserve the reference wording as a suggestion, e.g. unknown_9fe5 versus --center--.
export function suggestUnknownLabels<T extends MechanicRow>(
  matches: { reference: T; generated: T }[],
) {
  const groups = new Map<
    string,
    { id: string; referenceName: string; observations: { reference: T; generated: T }[] }
  >();
  for (const match of matches) {
    if (!/^unknown_[\da-f]+$/i.test(match.generated.name) || !match.reference.name) continue;
    for (const id of new Set(
      match.generated.ids.filter((id) => match.reference.ids.includes(id)),
    )) {
      const key = JSON.stringify([id, match.reference.name]);
      const group = groups.get(key) ?? {
        id,
        referenceName: match.reference.name,
        observations: [],
      };
      group.observations.push(match);
      groups.set(key, group);
    }
  }
  return [...groups.values()];
}

export function compareRows<T extends MechanicRow>(
  reference: T[],
  generated: T[],
  options: { clockOffset?: number; maxTimeDelta?: number } = {},
) {
  const { clockOffset = 0, maxTimeDelta = Infinity } = options;
  assert(Number.isFinite(clockOffset) && maxTimeDelta >= 0 && !Number.isNaN(maxTimeDelta));
  // Tether IDs and ability IDs occupy separate namespaces, e.g. 0175 can occur in both.
  const sameNamespace = (a: T, b: T) =>
    !a.evidence || !b.evidence || (a.evidence.type === "Tether") === (b.evidence.type === "Tether");
  const equivalent = (a: T, b: T) =>
    Math.abs(b.time - a.time - clockOffset) <= maxTimeDelta &&
    sameNamespace(a, b) &&
    (a.ids.length && b.ids.length ? a.ids.some((id) => b.ids.includes(id)) : a.name === b.name);
  // Prefer the closest ordered alignment among equally complete matches, e.g. repeated spotlight waves.
  const lengths = Array.from(
    { length: reference.length + 1 },
    () => new Uint32Array(generated.length + 1),
  );
  const costs = Array.from(
    { length: reference.length + 1 },
    () => new Float64Array(generated.length + 1),
  );
  for (let i = reference.length - 1; i >= 0; i--) {
    for (let j = generated.length - 1; j >= 0; j--) {
      let bestLength = lengths[i + 1][j],
        bestCost = costs[i + 1][j];
      const consider = (length: number, cost: number) => {
        if (length > bestLength || (length === bestLength && cost < bestCost)) {
          bestLength = length;
          bestCost = cost;
        }
      };
      consider(lengths[i][j + 1], costs[i][j + 1]);
      if (equivalent(reference[i], generated[j]))
        consider(
          lengths[i + 1][j + 1] + 1,
          costs[i + 1][j + 1] + Math.abs(generated[j].time - reference[i].time - clockOffset),
        );
      lengths[i][j] = bestLength;
      costs[i][j] = bestCost;
    }
  }
  const matched: {
    reference: T;
    generated: T;
    timeDelta: number;
    durationDelta: number;
    syncChanged: boolean;
  }[] = [];
  const missing: T[] = [],
    extra: T[] = [];
  let i = 0,
    j = 0;
  while (i < reference.length || j < generated.length) {
    if (
      i < reference.length &&
      j < generated.length &&
      equivalent(reference[i], generated[j]) &&
      lengths[i][j] === lengths[i + 1][j + 1] + 1 &&
      costs[i][j] ===
        costs[i + 1][j + 1] + Math.abs(generated[j].time - reference[i].time - clockOffset)
    ) {
      const a = reference[i++],
        b = generated[j++];
      matched.push({
        reference: a,
        generated: b,
        timeDelta: b.time - a.time,
        durationDelta: (b.duration ?? 0) - (a.duration ?? 0),
        syncChanged: JSON.stringify(a.sync) !== JSON.stringify(b.sync),
      });
    } else if (
      i < reference.length &&
      (j === generated.length ||
        lengths[i + 1][j] > lengths[i][j + 1] ||
        (lengths[i + 1][j] === lengths[i][j + 1] && costs[i + 1][j] <= costs[i][j + 1]))
    ) {
      missing.push(reference[i++]);
    } else {
      extra.push(generated[j++]);
    }
  }
  return { matched, missing, extra };
}

const canonical = (value: unknown): unknown =>
  Array.isArray(value)
    ? value.map(canonical).sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))
    : value && typeof value === "object"
      ? Object.fromEntries(
          Object.entries(value)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([key, item]) => [key, canonical(item)]),
        )
      : value;

export async function compareFiles(
  generatedFile: string,
  referenceFile: string,
  consumer: string,
  options: { clockOffset?: number; maxTimeDelta?: number } = {},
) {
  const require = createRequire(path.join(consumer, "package.json"));
  const JSON5 = require("json5");
  const { TimelineParser, regexes } = await import(
    pathToFileURL(path.join(consumer, "ui/raidboss/timeline_parser.ts")).href
  );
  const parse = (file: string) => {
    const text = fs.readFileSync(file, "utf8");
    const lines = text.split("\n");
    const parser: ParsedTimeline = new TimelineParser(text, [], []);
    assert.deepEqual(parser.errors, [], `Consumer parser rejected ${file}`);
    const rows = parser.events.map((event) => {
      const original = lines[event.lineNumber - 1].trim();
      const line = original.slice(regexes.line.exec(original).groups.text.length).trim();
      const active = regexes.syncNetRegex.exec(line.split("#")[0])?.groups;
      // Commented syncs are editorial evidence only; they never become active consumer syncs.
      const comment = line.slice(line.indexOf("#") + 1);
      const disabled =
        !active && line.includes("#") ? regexes.syncNetRegex.exec(comment)?.groups : undefined;
      const evidence = active ?? disabled;
      const fields: Record<string, unknown> | undefined = evidence
        ? JSON5.parse(evidence.netRegex)
        : undefined;
      const fieldIds = fields?.id;
      const ids = (Array.isArray(fieldIds) ? fieldIds : fieldIds === undefined ? [] : [fieldIds])
        .filter((id: unknown): id is string => typeof id === "string" && /^[0-9a-f]+$/i.test(id))
        .map((id) => id.toUpperCase())
        .sort();
      return {
        name: event.name,
        time: event.time,
        duration: event.duration ?? 0,
        line: event.lineNumber,
        hidden: event.name === "" || Boolean(parser.ignores[event.name]),
        ids,
        evidence: evidence
          ? { type: evidence.netRegexType, fields: canonical(fields), disabled: Boolean(disabled) }
          : null,
        sync: event.sync
          ? {
              type: event.sync.regexType,
              input: canonical(event.sync.origInput),
              window: [event.time - event.sync.start, event.sync.end - event.time],
              jump: event.sync.jump ?? null,
              jumpType: event.sync.jumpType ?? null,
            }
          : null,
      };
    });
    const ignoredIds = lines
      .filter((line) => line.trim().startsWith("# -ii "))
      .flatMap((line) => line.trim().slice(6).trim().split(/\s+/))
      .map((id) => id.toUpperCase());
    return {
      file,
      rows,
      ignoredIds,
      forceJumps: parser.forceJumps.map((sync) => ({
        time: sync.time,
        jump: sync.jump,
        jumpType: sync.jumpType,
      })),
    };
  };
  const reference = parse(referenceFile),
    generated = parse(generatedFile);
  const comparison = compareRows(
    reference.rows.filter((row) => !row.hidden),
    generated.rows.filter((row) => !row.hidden),
    options,
  );
  return {
    cactbotCommit: execFileSync("git", ["-C", consumer, "rev-parse", "HEAD"], {
      encoding: "utf8",
    }).trim(),
    summary: {
      referenceRows: reference.rows.length,
      generatedRows: generated.rows.length,
      matched: comparison.matched.length,
      referenceOnly: comparison.missing.length,
      generatedOnly: comparison.extra.length,
      generatedIgnoredIds: [
        ...new Set(
          generated.rows
            .flatMap((row) => row.ids)
            .filter((id) => reference.ignoredIds.includes(id)),
        ),
      ],
    },
    reference,
    generated,
    comparison,
    diagnostics: {
      referenceOnlyEvidence: describeUnmatched(comparison.missing),
      generatedOnlyIgnoredRows: comparison.extra.filter((row) =>
        row.ids.some((id) => reference.ignoredIds.includes(id)),
      ).length,
      simultaneousMatches: simultaneousMatches(comparison.matched),
      unknownLabelCandidates: suggestUnknownLabels(comparison.matched),
    },
    alignment: {
      clockOffset: options.clockOffset ?? 0,
      maxTimeDelta: options.maxTimeDelta ?? null,
    },
    scope:
      "Official parser with base text; active/commented ID evidence, visible order, durations, windows and jumps. Equal-length alignments minimize clock-adjusted time difference; an optional limit rejects distant matches. Tether IDs are separate from ability IDs. Differences and raw clock deltas are diagnostic, not proof of branch or editorial parity; no locale replacements or ACT runtime.",
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      output: { type: "string", short: "o" },
      "clock-offset": { type: "string" },
      "max-time-delta": { type: "string" },
    },
  });
  assert(
    positionals.length === 3 && values.output,
    "Usage: compare_cactbot.ts GENERATED.txt REFERENCE.txt CACTBOT -o REPORT.json [--clock-offset SECONDS] [--max-time-delta SECONDS]",
  );
  const [generated, reference, consumer] = positionals.map((file) => path.resolve(file));
  const numberOption = (value: string | undefined) => {
    if (value === undefined) return undefined;
    assert(
      value.trim() !== "" && Number.isFinite(Number(value)),
      "Clock options must be finite numbers in seconds",
    );
    return Number(value);
  };
  const result = await compareFiles(generated, reference, consumer, {
    clockOffset: numberOption(values["clock-offset"]),
    maxTimeDelta: numberOption(values["max-time-delta"]),
  });
  fs.writeFileSync(values.output, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx" });
  console.log(JSON.stringify(result.summary));
}

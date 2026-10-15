// Compare consumer-parsed mechanics, e.g. see README's tooling workflow for the ts-node loader.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";
import { execFileSync } from "node:child_process";
import { parseArgs } from "node:util";
import type { ParsedTimeline } from "./types.ts";

type MechanicRow = { name: string; ids: string[]; time: number; duration?: number; sync?: unknown };

export function compareRows<T extends MechanicRow>(reference: T[], generated: T[]) {
  const equivalent = (a: T, b: T) =>
    a.ids.length && b.ids.length ? a.ids.some((id) => b.ids.includes(id)) : a.name === b.name;
  // Order-aware matching keeps repeated casts distinct; raw clock offsets remain diagnostic.
  const lengths = Array.from(
    { length: reference.length + 1 },
    () => new Uint32Array(generated.length + 1),
  );
  for (let i = reference.length - 1; i >= 0; i--)
    for (let j = generated.length - 1; j >= 0; j--)
      lengths[i][j] = equivalent(reference[i], generated[j])
        ? lengths[i + 1][j + 1] + 1
        : Math.max(lengths[i + 1][j], lengths[i][j + 1]);
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
    if (i < reference.length && j < generated.length && equivalent(reference[i], generated[j])) {
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
      (j === generated.length || lengths[i + 1][j] >= lengths[i][j + 1])
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

export async function compareFiles(generatedFile: string, referenceFile: string, consumer: string) {
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
    scope:
      "Official parser with base text; active/commented ID evidence, visible order, durations, windows and jumps. Overlapping ID alternatives can match. Differences and raw clock deltas are diagnostic, not proof of branch or editorial parity; no locale replacements or ACT runtime.",
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: { output: { type: "string", short: "o" } },
  });
  assert(
    positionals.length === 3 && values.output,
    "Usage: compare_cactbot.ts GENERATED.txt REFERENCE.txt CACTBOT -o REPORT.json",
  );
  const [generated, reference, consumer] = positionals.map((file) => path.resolve(file));
  const result = await compareFiles(generated, reference, consumer);
  fs.writeFileSync(values.output, `${JSON.stringify(result, null, 2)}\n`, { flag: "wx" });
  console.log(JSON.stringify(result.summary));
}

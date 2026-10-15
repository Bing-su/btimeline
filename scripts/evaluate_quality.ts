// Run the existing gates together, e.g. see README for a frozen-manifest comparison invocation.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { spawnSync, execFileSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import type {
  CollectedLog,
  EvaluationReport,
  GenerationReport,
  ReplayCoverageReport,
} from "./types.ts";
type Metrics = Partial<
  Record<
    | "eligible"
    | "represented"
    | "excluded"
    | "unrepresented"
    | "referenceOnly"
    | "generatedOnly"
    | "forecastRate"
    | "officialFailures"
    | "consumerErrors",
    number | null
  >
>;
type QualityGroup = {
  evaluation: string;
  group: { encounter: number; difficulty: number };
  status: string;
  passed: boolean;
  coverage?: ReturnType<typeof auditCoverage>;
  referenceSha256?: string;
  comparison?: { referenceOnly: number; generatedOnly: number };
  official?: { failures: unknown[] } | null;
  consumer?: {
    split: string;
    passed: boolean;
    results?: {
      excludedFromGeneration?: boolean;
      forecastTargets: number;
      forecasted: number;
      missing: unknown[];
      unexpected: unknown[];
    }[];
  }[];
  replayCoverage?: (ReturnType<typeof auditReplayCoverage> & { split: string })[];
  metrics?: Metrics;
  error?: string;
};
export type QualityVersion = {
  version: string;
  binarySha256: string;
  replayPassed: boolean;
  groups: QualityGroup[];
  error?: string;
};

const read = <T>(file: string): T => JSON.parse(fs.readFileSync(file, "utf8"));
const hash = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");

export function auditCoverage(generation: GenerationReport) {
  const inputs = generation.inputs ??
    generation.segments?.flatMap((segment) => segment.report.inputs ?? [segment.report]) ?? [
      generation,
    ];
  const coverage = generation.outputCoverage ?? [
    {
      file: generation.input!.file,
      representedEventIndices: generation.emittedEventIndices,
      excludedEventIndices: [],
      omittedEventIndices: [],
    },
  ];
  assert(coverage.length > 0 && (generation.slots?.length ?? 0) > 0, "Empty generation evidence");
  const files = coverage.map((row) => {
    const log = read<CollectedLog>(row.file);
    const fight = log.report.fights[0];
    const enemies = new Set(
      [...(fight.enemyNPCs ?? []), ...(fight.enemyPets ?? [])]
        .map((actor) => actor.id)
        .concat(fight.enemyPlayers ?? []),
    );
    const source = inputs.find((input) => input.input?.file === row.file);
    const represented = new Set<number>(row.representedEventIndices);
    // Count collapsed instances only through an emitted row, e.g. simultaneous helper casts.
    if (!generation.outputCoverage) {
      for (const cast of generation.collapsedCasts ?? []) {
        if (!represented.has(cast.representativeEventIndex)) continue;
        for (const index of cast.omittedEventIndices) represented.add(index);
      }
    }
    const excluded = new Set<number>(row.excludedEventIndices ?? []);
    assert(
      [...represented, ...excluded].every(
        (index) => Number.isInteger(index) && index >= 0 && index < log.events.length,
      ),
      "Coverage index outside raw input",
    );
    assert(
      [...represented].every((index) => !excluded.has(index)),
      "Represented/excluded coverage overlaps",
    );
    if (generation.mode !== "raid")
      assert((source?.bossSegments?.length ?? 0) > 0, "Missing dungeon boss scope");
    const eligible: number[] = [],
      outsideBossScope: number[] = [];
    log.events.forEach((event, index) => {
      if (event.type !== "cast" || event.melee || !enemies.has(event.sourceID!)) return;
      const at = event.timestamp - fight.startTime;
      if (
        generation.mode !== "raid" &&
        !source!.bossSegments!.some((span) => span.startMs <= at && at <= span.endMs)
      ) {
        outsideBossScope.push(index);
      } else {
        eligible.push(index);
      }
    });
    const unexplained = eligible.filter((index) => !represented.has(index) && !excluded.has(index));
    return {
      file: row.file,
      eligible: eligible.length,
      represented: eligible.filter((index) => represented.has(index)).length,
      excluded: eligible.filter((index) => excluded.has(index)).length,
      unexplained,
      reportedOmissions: row.omittedEventIndices ?? [],
      outsideBossScope,
    };
  });
  const totals = Object.fromEntries(
    (["eligible", "represented", "excluded"] as const).map((key) => [
      key,
      files.reduce((sum, file) => sum + file[key], 0),
    ]),
  );
  return {
    files,
    totals,
    passed:
      totals.eligible > 0 &&
      files.every((file) => file.unexplained.length === 0 && file.reportedOmissions.length === 0),
    scope:
      "Raw enemy non-melee casts in selected generation inputs. Dungeon/alliance casts are limited to reported boss spans; outside-scope indices remain explicit. Exclusions are not predictions; holdouts are checked separately by replay/consumer.",
  };
}

export function metricsDelta(baseline: Metrics, candidate: Metrics) {
  const fields = [
    "eligible",
    "represented",
    "excluded",
    "unrepresented",
    "referenceOnly",
    "generatedOnly",
    "forecastRate",
    "officialFailures",
    "consumerErrors",
  ] as const;
  return Object.fromEntries(
    fields.map((key) => {
      const before = baseline[key],
        after = candidate[key];
      return [key, typeof after === "number" && typeof before === "number" ? after - before : null];
    }),
  );
}

export function auditReplayCoverage(replay: ReplayCoverageReport) {
  assert(replay.pulls?.length > 0, "Empty replay evidence");
  const files = replay.pulls.map((pull) => {
    assert(
      Array.isArray(pull.unrepresentedEventIndices) && Array.isArray(pull.excludedEventIndices),
      "Missing replay raw-cast accounting",
    );
    return {
      file: pull.file,
      diagnostic: Boolean(pull.excludedFromGeneration),
      unrepresented: pull.unrepresentedEventIndices,
      excluded: pull.excludedEventIndices.length,
    };
  });
  return {
    files,
    passed: files.every((file) => file.diagnostic || file.unrepresented.length === 0),
  };
}

function main() {
  const { positionals, values } = parseArgs({
    allowPositionals: true,
    options: {
      binary: { type: "string", default: "target/release/btimeline" },
      baseline: { type: "string" },
      cactbot: { type: "string" },
      references: { type: "string" },
      mode: { type: "string", default: "raid" },
      "combine-encounters": { type: "boolean", default: false },
    },
  });
  assert(
    positionals.length === 2 && values.cactbot && values.references,
    "Usage: evaluate_quality.ts MANIFEST.csv OUTPUT --cactbot CHECKOUT --references MAP.json [--binary BINARY] [--baseline BINARY] [--mode raid|dungeon|alliance] [--combine-encounters]",
  );
  assert(
    values.mode && ["raid", "dungeon", "alliance"].includes(values.mode),
    "Invalid generation mode",
  );
  const [manifest, output] = positionals.map((file) => path.resolve(file));
  const consumer = path.resolve(values.cactbot),
    referencesFile = path.resolve(values.references);
  const references = read<Record<string, string>>(referencesFile);
  const script = (name: string) => fileURLToPath(new URL(name, import.meta.url));
  const consumerCommit = () =>
    execFileSync("git", ["-C", consumer, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const provenance = {
    manifestSha256: hash(manifest),
    referencesSha256: hash(referencesFile),
    cactbotCommit: consumerCommit(),
    mode: values.mode,
    combineEncounters: values["combine-encounters"],
  };
  assert(!fs.existsSync(output), "Quality output already exists");
  fs.mkdirSync(output, { recursive: true });
  const checks: { label: string; status: number | null; error: string | null }[] = [];
  const run = (label: string, command: string, args: string[], env = process.env) => {
    const result = spawnSync(command, args, { encoding: "utf8", env, maxBuffer: 32 * 1024 * 1024 });
    const check = { label, status: result.status, error: result.error?.message ?? null };
    checks.push(check);
    fs.writeFileSync(
      path.join(output, `${checks.length}.log`),
      `${label}\n${result.stdout ?? ""}\n${result.stderr ?? ""}`,
    );
    return !result.error && result.status === 0;
  };
  const env = {
    ...process.env,
    TS_NODE_TRANSPILE_ONLY: "true",
    TS_NODE_PROJECT: path.join(consumer, "tsconfig.json"),
  };
  // Bun resolves cactbot's extensionless TS imports directly; Node needs the checkout's loader.
  const consumerArgs = (file: string, args: string[]) => [
    ...(process.versions.bun
      ? []
      : ["--loader", path.join(consumer, "node_modules/ts-node/esm.mjs")]),
    script(file),
    ...args,
  ];
  const versions: QualityVersion[] = [];
  for (const [version, binary] of [
    ["baseline", values.baseline],
    ["candidate", values.binary],
  ] as const) {
    if (!binary) continue;
    const executable = path.resolve(binary),
      folder = path.join(output, version);
    const args = [
      script("evaluate_replay.nu"),
      manifest,
      folder,
      "--binary",
      executable,
      "--mode",
      values.mode,
    ];
    if (values["combine-encounters"]) args.push("--combine-encounters");
    const replayPassed = run(`${version}: replay`, "nu", args);
    const results: QualityVersion = {
      version,
      binarySha256: hash(executable),
      replayPassed,
      groups: [],
    };
    versions.push(results);
    if (!fs.existsSync(path.join(folder, "evaluation.json"))) {
      results.error = "Replay evaluation produced no report";
      continue;
    }
    const evaluation = read<EvaluationReport>(path.join(folder, "evaluation.json"));
    for (const group of evaluation.groups) {
      const result: QualityGroup = {
        evaluation: group.evaluation,
        group: group.group,
        status: group.status,
        passed: false,
      };
      results.groups.push(result);
      try {
        assert(group.directory, "Group did not produce generation artifacts");
        const yaml = path.join(group.directory, "draft.yaml"),
          text = path.join(group.directory, "draft.txt");
        assert(
          run(`${version}: validate`, executable, ["validate", yaml]),
          "Schema/semantic validation failed",
        );
        result.coverage = auditCoverage(read(path.join(group.directory, "draft.report.json")));
        assert(
          run(`${version}: convert`, executable, ["convert", yaml, "-o", text]),
          "Text conversion failed",
        );
        const key = path.basename(group.directory);
        assert(typeof references[key] === "string", `Missing cactbot reference mapping for ${key}`);
        const reference = path.resolve(path.dirname(referencesFile), references[key]);
        result.referenceSha256 = hash(reference);
        const comparisonFile = path.join(group.directory, "comparison.json");
        assert(
          run(
            `${version}: reference comparison`,
            process.execPath,
            consumerArgs("compare_cactbot.ts", [text, reference, consumer, "-o", comparisonFile]),
            env,
          ),
          "Reference comparison failed",
        );
        result.comparison = read<{ summary: QualityGroup["comparison"] }>(comparisonFile).summary;
        const officialFile = path.join(group.directory, "official.json");
        const officialPassed = run(`${version}: official tests`, process.execPath, [
          script("verify_official_cactbot.ts"),
          text,
          consumer,
          reference,
          officialFile,
        ]);
        result.official = fs.existsSync(officialFile) ? read(officialFile) : null;
        result.consumer = [];
        result.replayCoverage = [];
        for (const split of ["train", ...(group.holdout ? ["holdout"] : [])]) {
          const replay = path.join(group.directory, `${split}.replay.json`);
          result.replayCoverage.push({ split, ...auditReplayCoverage(read(replay)) });
          const passed = run(
            `${version}: ${split} consumer`,
            process.execPath,
            consumerArgs("verify_phase_schedule.ts", [yaml, consumer, replay]),
            env,
          );
          const file = replay.replace(/\.replay\.json$/, ".cactbot.json");
          result.consumer.push({
            split,
            passed,
            ...(fs.existsSync(file)
              ? read<NonNullable<QualityGroup["consumer"]>[number]>(file)
              : {}),
          });
        }
        const checked = result.consumer
          .flatMap((item) => item.results ?? [])
          .filter((pull) => !pull.excludedFromGeneration);
        const targets = checked.reduce((sum, pull) => sum + pull.forecastTargets, 0);
        result.metrics = {
          ...result.coverage.totals,
          ...result.comparison,
          unrepresented: result.replayCoverage
            .flatMap((item) => item.files)
            .filter((file) => !file.diagnostic)
            .reduce((sum, file) => sum + file.unrepresented.length, 0),
          forecastRate: targets
            ? checked.reduce((sum, pull) => sum + pull.forecasted, 0) / targets
            : null,
          officialFailures: result.official?.failures.length ?? null,
          consumerErrors: checked.reduce(
            (sum, pull) => sum + pull.missing.length + pull.unexpected.length,
            0,
          ),
        };
        result.passed =
          group.status === "passed" &&
          result.coverage.passed &&
          officialPassed &&
          targets > 0 &&
          result.replayCoverage.every((item) => item.passed) &&
          result.consumer.every((item) => item.passed);
      } catch (error) {
        result.error = error instanceof Error ? error.message : String(error);
      }
      // Preserve each completed group's evidence if the next group or process is interrupted.
      fs.writeFileSync(
        path.join(output, `${version}.quality.json`),
        `${JSON.stringify(results, null, 2)}\n`,
      );
    }
  }
  assert(
    hash(manifest) === provenance.manifestSha256 && consumerCommit() === provenance.cactbotCommit,
    "Evaluation provenance changed during the run",
  );
  const candidate = versions.find((version) => version.version === "candidate")!;
  const baseline = versions.find((version) => version.version === "baseline");
  const deltas = candidate.groups.map((group) => {
    const previous = baseline?.groups.find(
      (item) =>
        item.evaluation === group.evaluation &&
        JSON.stringify(item.group) === JSON.stringify(group.group),
    );
    return {
      evaluation: group.evaluation,
      group: group.group,
      delta:
        previous?.metrics && group.metrics ? metricsDelta(previous.metrics, group.metrics) : null,
    };
  });
  const passed =
    candidate.replayPassed &&
    candidate.groups.length > 0 &&
    candidate.groups.every((group) => group.passed);
  const report = {
    provenance,
    versions,
    deltas,
    checks,
    passed,
    scope:
      "Correctness gates require independent report holdouts, scoped raw cast accounting, official timeline tests and consumer sync checks. Reference differences, exclusions and forecast deltas require interpretation; passing does not establish human editorial parity or live ACT/browser compatibility.",
  };
  fs.writeFileSync(path.join(output, "quality.json"), `${JSON.stringify(report, null, 2)}\n`);
  const rows = versions.flatMap((version) =>
    version.groups.map(
      (group) =>
        `| ${version.version} | ${group.evaluation} | ${group.group.encounter}/${group.group.difficulty} | ${group.passed} | ${group.metrics?.eligible ?? "?"} | ${group.metrics?.excluded ?? "?"} | ${group.metrics?.referenceOnly ?? "?"} | ${group.metrics?.forecastRate?.toFixed(3) ?? "?"} | ${group.metrics?.officialFailures ?? "?"} | ${group.error?.replaceAll("|", "\\|") ?? ""} |`,
    ),
  );
  fs.writeFileSync(
    path.join(output, "quality.md"),
    `# Timeline quality evaluation\n\n| Version | Cohort | Encounter/difficulty | Gates passed | Raw casts | Excluded | Reference only | Forecast rate | Official failures | Error |\n| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |\n${rows.join("\n")}\n\n${report.scope}\n`,
  );
  console.log(JSON.stringify({ passed, report: path.join(output, "quality.json") }));
  process.exitCode = passed ? 0 : 1;
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href)
  main();

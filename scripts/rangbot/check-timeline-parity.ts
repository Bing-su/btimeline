// Run from Rangbot through Bun's stdin so its TypeScript aliases and dependencies resolve there.
import { execFile } from "node:child_process";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { promisify } from "node:util";

import { timelineDocument } from "$src/engine/timeline/timeline-document";
import { recordingSchema, replayTimelineRecording } from "$src/engine/timeline/timeline-recording";
import { timelineRuntime } from "$src/engine/timeline/timeline-runtime";
import { z } from "zod";

const execute = promisify(execFile);
const rustOutputSchema = z.object({
  recordings: z.record(
    z.string(),
    z.array(
      z.object({
        at_ms: z.number(),
        action: z.string(),
        event: z.object({ id: z.string() }).optional(),
      }),
    ),
  ),
});

const forecastSchema = z.object({
  phase: z.string().nullable(),
  events: z.array(z.object({ id: z.string(), at_ms: z.number() })),
  alternatives: z.array(
    z.object({
      choice_id: z.string(),
      choice_at_ms: z.number(),
      option_id: z.string(),
      at_ms: z.number(),
      state: z.string(),
      omitted: z.number(),
      events: z.array(z.object({ id: z.string(), at_ms: z.number() })),
    }),
  ),
});

function futureEvents(rows: ReturnType<typeof timelineRuntime.project>["rows"], cutoff: number) {
  return rows
    .filter((row) => !row.active && row.remainingMs > 0)
    .map((row) => ({ id: row.eventId, at_ms: cutoff + row.remainingMs }));
}

function checkAlternatives(
  projection: ReturnType<typeof timelineRuntime.project>,
  forecast: z.infer<typeof forecastSchema>,
  filePath: string,
  cutoff: number,
) {
  const expected = projection.alternatives.map((option) => ({
    choice_id: option.choiceId,
    choice_at_ms: option.choiceAtMs,
    option_id: option.optionId,
    at_ms: option.atMs,
    state: option.state,
    omitted: option.omitted,
    events: futureEvents(option.rows, cutoff),
  }));
  if (JSON.stringify(expected) !== JSON.stringify(forecast.alternatives)) {
    throw new Error(`${filePath}: alternatives mismatch at ${cutoff}`);
  }
}

async function checkForecasts(
  executable: string,
  filePath: string,
  inputPath: string,
  document: ReturnType<typeof timelineDocument.read>,
  input: z.infer<typeof recordingSchema>,
) {
  await Promise.all(
    [...new Set(input.observations.map((observation) => observation.at_ms))].map(async (cutoff) => {
      const { frame } = replayTimelineRecording(document, input, cutoff);
      const query = async (maxRows: number) => {
        const { stdout } = await execute(executable, [
          "forecast",
          filePath,
          inputPath,
          "--at-ms",
          String(cutoff),
          "--horizon-ms",
          "10000",
          "--max-rows",
          String(maxRows),
        ]);
        return forecastSchema.parse(JSON.parse(stdout));
      };
      const forecast = await query(10000);
      if (forecast.phase !== (frame.phaseId ?? null)) {
        throw new Error(`${filePath}: phase mismatch at ${cutoff}`);
      }
      const projection = timelineRuntime.project(frame, { horizonMs: 10000, maxRows: 10000 });
      checkAlternatives(projection, forecast, filePath, cutoff);
      const expected = futureEvents(projection.rows, cutoff);
      if (JSON.stringify(forecast.events) !== JSON.stringify(expected)) {
        throw new Error(
          `${filePath}: forecast mismatch at ${cutoff}\nRust: ${JSON.stringify(forecast.events)}\nRangbot: ${JSON.stringify(expected)}`,
        );
      }
      // An active or exact-cutoff row can occupy the overlay's first slot before the future rows.
      const first = projection.rows[0];
      if (expected.length > 1 && first !== undefined && !first.active && first.remainingMs > 0) {
        const bounded = await query(1);
        const limited = timelineRuntime.project(frame, { horizonMs: 10000, maxRows: 1 });
        if (JSON.stringify(bounded.events) !== JSON.stringify(futureEvents(limited.rows, cutoff))) {
          throw new Error(`${filePath}: one-row forecast mismatch at ${cutoff}`);
        }
      }
    }),
  );
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length !== 2 || args[0] !== "--btimeline") {
    throw new Error(
      'Usage from btimeline: bun --no-install --cwd ../rangbot - --btimeline "$PWD/target/debug/btimeline" < scripts/rangbot/check-timeline-parity.ts',
    );
  }
  const executable = path.resolve(z.string().parse(args[1]));
  const directory = "tests/fixtures/timeline-contract";
  const files = (await readdir(directory)).filter((file) => file.endsWith(".kdl"));
  await Promise.all(
    files.map(async (file) => {
      const filePath = `${directory}/${file}`;
      const inputPath = filePath.replace(/\.kdl$/, ".json");
      const input = recordingSchema.parse(JSON.parse(await readFile(inputPath, "utf8")));
      const source = await readFile(filePath, "utf8");
      const document = timelineDocument.read(source);
      const { events } = replayTimelineRecording(document, input);
      const { stdout } = await execute(executable, ["compare", filePath, inputPath, "--json"]);
      const result = rustOutputSchema.parse(JSON.parse(stdout));
      const actual = Object.values(result.recordings)
        .flat()
        .flatMap((row) =>
          row.action === "event" && row.event !== undefined
            ? [{ eventId: row.event.id, atMs: row.at_ms }]
            : [],
        );
      if (JSON.stringify(actual) !== JSON.stringify(events)) {
        throw new Error(
          `${file}: runtime mismatch\nRust: ${JSON.stringify(actual)}\nRangbot: ${JSON.stringify(events)}`,
        );
      }
      await checkForecasts(executable, filePath, inputPath, document, input);
      process.stdout.write(
        `${file}: ${events.length} events, phase checkpoints and forecasts agree\n`,
      );
    }),
  );
}

await main();

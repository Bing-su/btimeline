// Compare generated loop forecasts with raw casts, including traces omitted from the fitting input.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const [fitFile, observedFile, binary, output] = process.argv.slice(2);
assert(
  fitFile && observedFile && binary && output,
  "Usage: replay_dungeon_loops.ts FIT.evidence.json OBSERVED.evidence.json BTIMELINE REPORT.json",
);
const fit = JSON.parse(await fs.readFile(fitFile, "utf8"));
const observed = JSON.parse(await fs.readFile(observedFile, "utf8"));
const timeline = fitFile.replace(/\.evidence\.json$/, ".kdl");
assert.notEqual(timeline, fitFile, "Expected a .evidence.json file");
assert.equal(
  createHash("sha256")
    .update(await fs.readFile(timeline))
    .digest("hex"),
  fit.timeline_sha256,
);
const directory = await fs.mkdtemp(path.join(os.tmpdir(), "btimeline-loop-replay-"));
const results = [];
try {
  for (const loop of fit.loops.filter((loop: any) => loop.status === "rendered")) {
    const raw = observed.loops.find(
      (row: any) => row.source === loop.source && row.status === "rendered",
    );
    assert(raw, `Missing raw episodes for ${loop.source}`);
    // The observed corpus supplies raw nodes, never held-out timestamps for the fitted schedule.
    for (const key of ["prefix", "body", "blocks"])
      assert.deepEqual(
        raw.model[key],
        loop.model[key],
        `Different observed ${key}: inspect separately`,
      );
    const definitions = new Map<string, any>(loop.events.map((event: any) => [event.event, event]));
    const fitted = new Set(loop.model.episodes.map((episode: any) => episode.trace));
    for (const episode of raw.model.episodes) {
      const input = observed.analysis.inputs.find((input: any) => input.trace === episode.trace);
      const graph = observed.progression.traces.find((trace: any) => trace.trace === episode.trace);
      const scope =
        loop.source === null
          ? { end_ms: input.end_ms }
          : observed.alignment_diagnostics
              .find(
                (row: any) =>
                  row.method === "boss-attempt-scope-v1" &&
                  row.trace === episode.trace &&
                  row.source === loop.source,
              )
              ?.attempts.find(
                (attempt: any) =>
                  attempt.actor === episode.actor && attempt.instance === episode.instance,
              );
      assert(scope, `Missing observed scope for ${episode.trace}`);
      const end = scope.end_ms ?? scope.last_cast_ms;
      const recording = path.join(directory, "recording.json");
      const samples = graph.nodes
        .filter((node: any) => node.activity.kind === "cast")
        .map((node: any) => ({
          at: input.observations[node.observation].at_ms,
          signal: node.activity,
          unclassified: node.unclassified === true,
        }))
        .filter((row: any) => row.at >= episode.phase_origin_ms && row.at <= end);
      await fs.writeFile(
        recording,
        JSON.stringify({
          version: 1,
          end_ms: end - episode.phase_origin_ms,
          observations: [
            ...(fit.mode === "raid"
              ? [{ at_ms: 0, type: "InCombat", fields: { inGameCombat: "1" } }]
              : []),
            ...samples.map((row: any) => ({
              at_ms: row.at - episode.phase_origin_ms,
              type: "Ability",
              fields: {
                id: row.signal.identifier.Ability.toString(16).toUpperCase(),
                // Unknown names remain absent fields, e.g. anonymous NPCs cannot match a named sync.
                ...(row.signal.source === null ? {} : { source: row.signal.source }),
              },
            })),
          ],
        }),
      );
      const compared = JSON.parse(
        execFileSync(binary, ["compare", timeline, recording, "--json"], { encoding: "utf8" }),
      );
      const rows = compared.recordings[recording];
      const events = rows
        .filter((row: any) => row.action === "event")
        .map((row: any) => ({ ...row, definition: definitions.get(row.detail.split(":")[0]) }))
        .filter((row: any) => row.definition);
      // Helper forecasts are observable predictions without making the helper a runtime sync.
      const helperMembers = (raw.model.helper_associations ?? [])
        .flatMap((association: any) => association.observations)
        .filter(
          (row: any) =>
            row.trace === episode.trace &&
            row.actor === episode.actor &&
            row.instance === episode.instance,
        )
        .flatMap((row: any) => row.members);
      const errors = [...episode.waves.flatMap((wave: any) => wave.members), ...helperMembers].map(
        (member: any) => {
          const matches = events.filter((event: any) =>
            event.definition.activities.some(
              (signal: any) => JSON.stringify(signal) === JSON.stringify(member.signal),
            ),
          );
          return matches.length
            ? Math.min(
                ...matches.map((event: any) =>
                  Math.abs(event.at_ms - (member.at_ms - episode.phase_origin_ms)),
                ),
              )
            : null;
        },
      );
      results.push({
        source: loop.source,
        trace: episode.trace,
        actor: episode.actor,
        instance: episode.instance,
        role: fitted.has(episode.trace) ? "fitted" : "holdout",
        observed_members: errors.length,
        associated_helper_members: helperMembers.length,
        unclassified_casts: samples.filter((sample: any) => sample.unclassified).length,
        unnamed_sources: samples.filter((sample: any) => sample.signal.source === null).length,
        no_prediction: errors.filter((error: number | null) => error === null).length,
        max_prediction_error_ms: Math.max(
          0,
          ...errors.filter((error: number | null) => error !== null),
        ),
        over_5_seconds: errors.filter((error: number | null) => error === null || error > 5000)
          .length,
        diagnostics: rows.filter((row: any) =>
          ["ambiguous", "unresolved", "violation", "skipped"].includes(row.action),
        ),
      });
    }
  }
  await fs.writeFile(
    output,
    JSON.stringify(
      {
        fitFile,
        observedFile,
        results,
        scope:
          "Raw cast-to-Ability projection per observed body span or raid combat, including associated helper forecasts. Raid start is a synthetic InCombat observation. Closest same-signal emitted forecast per raw member; not a one-to-one path validator. Shared observed structure is required before comparing timing. No ACT capture, death/reset or conditional-path proof.",
      },
      null,
      2,
    ) + "\n",
  );
  console.log(
    JSON.stringify({
      episodes: results.length,
      holdouts: results.filter((row) => row.role === "holdout").length,
      noPrediction: results.reduce((n, row) => n + row.no_prediction, 0),
      maxPredictionErrorMs: Math.max(0, ...results.map((row) => row.max_prediction_error_ms)),
      over5Seconds: results.reduce((n, row) => n + row.over_5_seconds, 0),
      diagnostics: results.reduce((n, row) => n + row.diagnostics.length, 0),
    }),
  );
} finally {
  await fs.rm(directory, { recursive: true, force: true });
}

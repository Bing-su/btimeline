import assert from "node:assert/strict";
import { griffinRecording, holdoutSummary, InputExclusion } from "./verify_griffin_holdout.ts";

const names = { B0EB: "Thunderspark", B0EC: "High Volts", B0F4: "Electrifying Flight" };
const cast = (timestamp: number, sourceID: number, abilityGameID: number) => ({
  timestamp,
  type: "cast",
  sourceID,
  abilityGameID,
});
const raw = {
  report: {
    masterData: {
      actors: [
        { id: 1, name: "Player" },
        { id: 2, name: "Thundergust Griffin" },
        { id: 3, name: "Shock Storm" },
      ],
    },
  },
  events: [
    cast(100, 1, 1),
    cast(200, 1, 0xb0eb),
    cast(300, 2, 0xb0eb),
    { timestamp: 400, type: "damage", targetID: 1, targetResources: { hitPoints: 0 } },
    cast(500, 2, 0xb0ec),
    cast(600, 3, 0xb0f4),
    { timestamp: 700, type: "damage", targetID: 2, targetResources: { hitPoints: 0 } },
    cast(800, 2, 0xb0ec),
  ],
};
const before = JSON.stringify(raw);
const recording = griffinRecording(raw, names);
assert.equal(recording.entry_ms, 200);
assert.equal(recording.end_ms, 600);
assert.equal(recording.completion_event, 6);
assert.equal(recording.observations.length, 6);
assert.equal(recording.observations[1].fields.source, "Player");
assert.deepEqual(recording.samples, [
  [0, "B0EB", 2],
  [200, "B0EC", 4],
  [300, "B0F4", 5],
]);
// A kill flag cannot replace the frozen endpoint evidence, even when the remaining casts look complete.
assert.throws(
  () => griffinRecording({ ...raw, events: raw.events.filter((_, index) => index !== 6) }, names),
  (error: any) =>
    error instanceof InputExclusion &&
    error.evidence.entry_event === 2 &&
    error.evidence.last_damage_event === -1,
);
assert.throws(
  () => griffinRecording({ ...raw, events: raw.events.filter((_, index) => index !== 2) }, names),
  /Missing Griffin entry/,
);
assert.throws(
  () => griffinRecording({ ...raw, events: [cast(100, 999, 1), ...raw.events] }, names),
  (error: any) =>
    error instanceof InputExclusion &&
    error.evidence.raw_index === 0 &&
    error.evidence.source_id === 999,
);
assert.equal(JSON.stringify(raw), before);
const valid = { status: "evaluated", passed: true };
const excluded = { status: "excluded", passed: null };
assert.deepEqual(holdoutSummary([valid, excluded]), {
  selected: 2,
  excluded: 1,
  eligible: 1,
  passed_traces: 1,
  passed: true,
});
assert.equal(holdoutSummary([excluded]).passed, false);
assert.equal(holdoutSummary([valid, { status: "evaluated", passed: false }]).passed, false);
assert.equal(holdoutSummary([valid, { status: "error", passed: false }]).passed, false);
console.log("Griffin holdout projection preserves noise and rejects missing endpoint evidence");

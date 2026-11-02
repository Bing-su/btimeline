import assert from "node:assert/strict";
import { mistwakeRecording } from "./mistwake_recording.ts";
import { InputExclusion } from "./verify_griffin_holdout.ts";

const cast = (timestamp: number, sourceID: number, abilityGameID: number) => ({
  timestamp,
  type: "cast",
  sourceID,
  abilityGameID,
});
const dead = (timestamp: number, targetID: number) => ({
  timestamp,
  type: "damage",
  targetID,
  targetResources: { hitPoints: 0 },
});
const raw = {
  report: {
    masterData: {
      actors: [
        { id: 1, name: "Treno Catoblepas" },
        { id: 2, name: "Amdusias" },
        { id: 3, name: "Thundergust Griffin" },
        { id: 4, name: "Shock Storm" },
        { id: 5, name: "Player" },
      ],
    },
  },
  events: [
    cast(0, 5, 0xa93f),
    cast(100, 1, 0xa93f),
    cast(200, 1, 0xa943),
    dead(300, 1),
    cast(400, 2, 0xb11d),
    cast(500, 2, 0xb11f),
    cast(501, 2, 0xb11f),
    dead(600, 2),
    cast(700, 3, 0xb0eb),
    cast(800, 4, 0xb0f4),
    dead(900, 3),
  ],
};
const original = JSON.stringify(raw);
const projected = mistwakeRecording(raw);
assert.equal(projected.observations.length, 8);
assert.deepEqual(
  projected.bosses.map((boss) => [boss.phase, boss.entry_ms, boss.end_ms, boss.targets.length]),
  [
    ["treno", 100, 300, 2],
    ["amdusias", 400, 600, 2],
    ["griffin", 700, 900, 2],
  ],
);
assert.equal(projected.bosses[1]!.targets[1].raw_index, 5);
assert.equal(projected.bosses[2]!.targets[1].fields.source, "Shock Storm");
assert.throws(
  () => mistwakeRecording({ ...raw, events: raw.events.filter((_, i) => i !== 7) }),
  InputExclusion,
);
assert.throws(
  () =>
    mistwakeRecording({ ...raw, events: raw.events.map((e, i) => (i === 3 ? dead(450, 1) : e)) }),
  /Overlapping or out-of-order/,
);
assert.equal(JSON.stringify(raw), original);
console.log(
  "Mistwake projection preserves observations, collapses target packets and rejects incomplete boss scope",
);

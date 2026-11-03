import assert from "node:assert/strict";
import { varisP3Recording } from "./varis_p3_recording.ts";
import { InputExclusion } from "./types.ts";

const ability = (timestamp: number, sourceID: number, abilityGameID: number, type = "cast") => ({
  timestamp,
  sourceID,
  abilityGameID,
  type,
});
const raw = {
  report: {
    fights: [{ endTime: 202000 }],
    masterData: {
      actors: [
        { id: 1, name: "bladesblood" },
        { id: 2, name: "Varis yae Galvus" },
        { id: 3, name: "gunshield" },
        { id: 4, name: "Ignis Est" },
      ],
    },
  },
  events: [
    ability(0, 2, 0x4cd7, "begincast"),
    ability(100, 1, 0x4ccf),
    ability(200, 2, 0x4cd8, "begincast"),
    ability(5200, 2, 0x4cd8),
    ability(22000, 4, 0x4cb6),
    ability(60000, 2, 0x4cd9, "begincast"),
    ability(65000, 2, 0x4cd9),
    ability(120000, 2, 0x4cd7, "begincast"),
    ability(125000, 2, 0x4cd7),
    ability(180000, 2, 0x4ce0),
    ability(190500, 3, 0x4ce4),
    ability(190501, 3, 0x4ce4),
    ability(199200, 3, 0x4ce3),
  ],
};
const original = JSON.stringify(raw);
const projected = varisP3Recording(raw);
assert.equal(projected.order, "LRE");
assert.equal(projected.origin_timestamp_ms, 100);
assert.equal(projected.observations[0].fields.source, "Bladesblood");
assert.equal(projected.observations[1].fields.source, "Varis Yae Galvus");
assert.deepEqual(
  projected.blocks.map((block) => [block.phase, block.targets.length]),
  [
    ["L", 2],
    ["LR", 1],
    ["final-E", 1],
    ["joined", 2],
  ],
);
assert.equal(projected.blocks[0]!.targets[0].lead_ms, 3000);
assert.equal(projected.blocks[0]!.targets[1].lead_ms, 5000);
assert.equal(projected.blocks[3]!.targets[0].fields.source, "Gunshield");
assert.equal(projected.observations.length, raw.events.length - 1);
assert.throws(
  () => varisP3Recording({ ...raw, events: raw.events.filter((_, index) => index !== 9) }),
  InputExclusion,
);
assert.throws(
  () =>
    varisP3Recording({
      ...raw,
      events: raw.events.map((event, index) =>
        index === 7 ? ability(120000, 2, 0x4cd8, "begincast") : event,
      ),
    }),
  /three distinct choices/,
);
assert.equal(JSON.stringify(raw), original);
console.log("Varis P3 scope, source aliases, history and shared alternative targets verified");

import assert from "node:assert/strict";
import { clyteumRecording } from "./clyteum_recording.ts";
import { InputExclusion } from "./verify_griffin_holdout.ts";

const cast = (timestamp: number, sourceID: number, abilityGameID: number) => ({
  timestamp,
  type: "cast",
  sourceID,
  abilityGameID,
});
const health = (timestamp: number, targetID: number, hitPoints: number) => ({
  timestamp,
  type: "damage",
  targetID,
  targetResources: { hitPoints, maxHitPoints: 100 },
});
const raw = {
  report: {
    masterData: {
      actors: [
        { id: 1, name: "Eye of the Scorpion", subType: "Boss" },
        { id: 2, name: "Chort", subType: "Boss" },
        { id: 3, name: "Malphas", subType: "Boss" },
        { id: 4, name: "", subType: "Unknown" },
      ],
    },
  },
  events: [
    cast(0, 4, 1),
    cast(100, 1, 0xbf00),
    cast(200, 1, 0xc3ff),
    cast(201, 1, 0xc400),
    health(300, 1, 0),
    cast(350, 4, 1),
    cast(400, 2, 0xbef4),
    cast(500, 2, 0xbee5),
    health(600, 2, 0),
    cast(700, 3, 0xbf18),
    cast(800, 3, 0xc489),
    health(900, 3, 0),
  ],
};
const original = JSON.stringify(raw);
const projected = clyteumRecording(raw);
assert.deepEqual(
  projected.bosses.map((boss) => [boss.phase, boss.entry_ms, boss.end_ms, boss.targets.length]),
  [
    ["eye", 100, 300, 2],
    ["chort", 400, 600, 2],
    ["malphas", 700, 900, 2],
  ],
);
assert.equal(projected.bosses[0]!.targets[1].name, "Petrifying Beam");
assert.equal(projected.bosses[1]!.targets[1].name, "Mortifying Flesh");
assert.equal(projected.observations.length, 7);
// A health reset can join two attempts into a convincing but false long cycle; reject that ambiguity.
assert.throws(
  () =>
    clyteumRecording({
      ...raw,
      events: [...raw.events, health(250, 1, 70), health(260, 1, 95)].toSorted(
        (a, b) => a.timestamp - b.timestamp,
      ),
    }),
  /Possible attempt reset/,
);
assert.throws(
  () =>
    clyteumRecording({
      ...raw,
      events: [...raw.events, cast(250, 4, 1)].toSorted((a, b) => a.timestamp - b.timestamp),
    }),
  /Unknown cast source in boss scope/,
);
assert.throws(
  () => clyteumRecording({ ...raw, events: raw.events.filter((_, index) => index !== 8) }),
  InputExclusion,
);
assert.equal(JSON.stringify(raw), original);
console.log(
  "Clyteum boss scopes, alias waves, unknown sources and possible attempt resets verified",
);

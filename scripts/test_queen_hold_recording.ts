import assert from "node:assert/strict";
import { queenHoldRecording } from "./queen_hold_recording.ts";
import { InputExclusion } from "./verify_griffin_holdout.ts";

const ability = (timestamp: number, abilityGameID: number, type = "cast", sourceID = 1) => ({
  timestamp,
  abilityGameID,
  type,
  sourceID,
});
const body = (at: number) => [
  ability(at, 0x8efa),
  ability(at + 15100, 0x8f20),
  ability(at + 18200, 0x8f23),
  ability(at + 26300, 0x8efb),
  ability(at + 43500, 0x8f21),
  ability(at + 46600, 0x8f22),
];
const raw = {
  report: {
    masterData: {
      actors: [
        { id: 1, name: "Queen Eternal" },
        { id: 2, name: "Player" },
      ],
    },
  },
  events: [
    ability(0, 0x8f00),
    ...body(54700),
    ...body(115300),
    ability(165000, 0x8eff, "begincast"),
    ability(170000, 0x8eff),
    ability(178100, 0x8f05),
    ability(185800, 0x8f08),
    ability(185801, 0x8f09),
    ability(233700, 0x8f00),
  ],
};
const original = JSON.stringify(raw);
const projected = queenHoldRecording(raw);
assert.equal(projected.exit_ms, 165000);
assert.equal(projected.end_ms, 233700);
assert.deepEqual(
  projected.hold_openings.map((row: any) => row[0]),
  [54700, 115300],
);
assert.equal(projected.complete_bodies, 2);
assert.equal(projected.observed_recurrences, 1);
assert.equal(projected.targets.filter((target: any) => target.kind === "besiegement").length, 1);
assert.equal(projected.observations.length, raw.events.length);
assert.equal(projected.targets[0].lead_ms, null);
assert.equal(projected.targets.find((target: any) => target.kind === "exit-shift").lead_ms, 3000);
assert.throws(
  () =>
    queenHoldRecording({
      ...raw,
      events: raw.events.filter((event) => event.type !== "begincast"),
    }),
  InputExclusion,
);
assert.throws(
  () => queenHoldRecording({ ...raw, events: raw.events.slice(0, -1) }),
  /post-exit continuation/,
);
assert.throws(
  () =>
    queenHoldRecording({
      ...raw,
      events: raw.events.map((event) =>
        event.type === "begincast" ? { ...event, sourceID: 2 } : event,
      ),
    }),
  /observed hold exit/,
);
assert.equal(JSON.stringify(raw), original);
console.log(
  "Queen hold projection preserves packets, counts complete bodies and requires a real exit with continuation",
);

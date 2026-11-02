import assert from "node:assert/strict";
import { sandoriaRecording } from "./sandoria_recording.ts";

const cast = (timestamp: number, sourceID: number, abilityGameID: number, type = "cast") => ({
  timestamp,
  type,
  sourceID,
  abilityGameID,
});
const raw = {
  report: {
    fights: [{ encounterID: 2068, endTime: 260000 }],
    masterData: {
      actors: [
        { id: 1, name: "Omega, the One", subType: "Boss" },
        { id: 2, name: "Ultima, the Feared", subType: "Boss" },
      ],
    },
  },
  events: [
    cast(0, 1, 0xad2b),
    cast(10000, 2, 0xad08, "begincast"),
    cast(12000, 2, 0xad08),
    cast(12001, 2, 0xad08),
    cast(12002, 1, 0xb087),
    cast(240000, 2, 0xad09),
  ],
};
const before = JSON.stringify(raw);
const result = sandoriaRecording(raw);
assert.equal(result.bosses.length, 1);
assert.equal(result.bosses[0]!.phase, "duo");
assert.equal(result.bosses[0]!.end_ms, 238000);
assert.deepEqual(
  result.bosses[0]!.targets.map((t) => t.name),
  ["Omega: Ion Efflux", "Ultima: Energy Orb", "Omega: Anti-Personnel Missile"],
);
assert.equal(result.observations[1]!.type, "StartsUsing");
assert.equal(result.observations.length, 5);
assert.throws(
  () => sandoriaRecording({ ...raw, events: [...raw.events, cast(13000, 999, 1)] }),
  /Unknown cast source/,
);
assert.throws(
  () => sandoriaRecording({ ...raw, events: raw.events.slice(1) }),
  /Missing boss entry/,
);
assert.throws(
  () =>
    sandoriaRecording({
      ...raw,
      events: [
        ...raw.events,
        ...[70, 95].map((hp, i) => ({
          type: "damage",
          timestamp: 14000 + i,
          targetID: 1,
          targetResources: { hitPoints: hp, maxHitPoints: 100 },
        })),
      ],
    }),
  /Possible attempt reset/,
);
const kirin = {
  report: {
    fights: [{ encounterID: 2067, endTime: 200000 }],
    masterData: { actors: [{ id: 1, name: "Faithbound Kirin", subType: "Boss" }] },
  },
  events: [
    cast(0, 1, 0xadca),
    cast(95000, 1, 0xb07b, "begincast"),
    cast(98000, 1, 0xb07b),
    cast(103100, 1, 0xb07c),
  ],
};
assert.deepEqual(
  sandoriaRecording(kirin).bosses[0]!.targets.map((t) => t.lead_ms ?? 5000),
  [5000, 2000, 5000],
);
assert.equal(JSON.stringify(raw), before);
console.log(
  "Sandoria simultaneous scope, censored wipes, selection lead and input rejection verified",
);

import assert from "node:assert/strict";
import { assertNormalFight, symmetricBody, trials } from "./audit_normal_trials.ts";
import { completeCycles, type Pattern, type Observation } from "./audit_dungeon_loops.ts";

for (const trial of ["doomtrain", "enuo"] as const) {
  const identity = trials[trial];
  const normal = {
    encounterID: identity.encounter,
    difficulty: 100,
    gameZone: { id: identity.zone },
  };
  assertNormalFight(normal, trial);
  // FFLogs labels the extreme zone's sole difficulty Normal too; the encounter/zone pair is essential.
  assert.throws(() =>
    assertNormalFight(
      { ...normal, encounterID: identity.extremeEncounter, gameZone: { id: identity.extremeZone } },
      trial,
    ),
  );
  assert.throws(() =>
    assertNormalFight({ ...normal, gameZone: { id: identity.extremeZone } }, trial),
  );
  assert.throws(() =>
    assertNormalFight({ encounterID: identity.encounter, difficulty: 100 }, trial),
  );
  assert.throws(() => assertNormalFight({ ...normal, difficulty: 101 }, trial));
}
const p = (id: string, time: number, kind: "cast" | "begincast" = "cast"): Pattern => ({
  ids: [id],
  sources: ["Boss"],
  time,
  name: id,
  kind,
});
const row = (
  id: string,
  at: number,
  node: number,
  kind: "cast" | "begincast" = "cast",
  entity = "1/1",
): Observation => ({ id, at, node, kind, source: "Boss", entities: [entity] });
const body = [p("A", 0), p("B", 1)];
const samples = [
  row("A", 0, 0),
  row("B", 1000, 1),
  row("A", 1500, 2, "begincast"),
  row("A", 2500, 3),
];
assert.equal(completeCycles(samples, body, p("A", 1.5, "begincast")).length, 1);
assert.equal(
  completeCycles(
    samples.filter((row) => row.kind !== "begincast"),
    body,
    p("A", 1.5, "begincast"),
  ).length,
  0,
);
const coincident = [
  row("A", 0, 0),
  row("A", 50, 1, "cast", "2/1"),
  row("B", 1000, 2),
  row("A", 2000, 3),
];
assert.deepEqual(completeCycles(coincident, body, p("A", 2))[0].body_nodes, [0, 1, 2]);
assert.deepEqual(
  completeCycles(
    coincident.map((row) => ({ ...row, entities: ["1/1"] })),
    body,
    p("A", 2),
  )[0].body_nodes,
  [1, 2],
);
assert.equal(symmetricBody([p("A", 0), p("A", 3), p("A", 6)]), true);
assert.equal(symmetricBody([p("A", 0), p("B", 1), p("A", 2), p("B", 3)]), true);
assert.equal(symmetricBody([p("A", 0), p("B", 1), p("A", 2), p("C", 3)]), false);
console.log("normal trial identity and loop projection tests passed");

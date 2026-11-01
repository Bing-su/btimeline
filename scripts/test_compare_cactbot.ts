// Test repeated mechanics and ID alternatives without requiring a game or network connection.
import assert from "node:assert/strict";
import {
  compareRows,
  describeUnmatched,
  simultaneousMatches,
  suggestUnknownLabels,
} from "./compare_cactbot.ts";
const row = (name: string, ids: string[], time = 0, duration = 0) => ({
  name,
  ids,
  time,
  duration,
  sync: null,
});
const result = compareRows(
  [
    row("One", ["A"], 1),
    row("Repeat", ["B"], 2),
    row("Repeat", ["B"], 3),
    row("Missing", ["C"], 4),
  ],
  [row("Translated", ["A", "D"], 101), row("Extra", ["E"], 102), row("Repeat", ["B"], 103, 2)],
);
assert.equal(result.matched.length, 2);
assert.equal(result.matched[0].timeDelta, 100);
assert.equal(result.matched[1].durationDelta, 2);
assert.deepEqual(
  result.missing.map((row) => row.name),
  ["Repeat", "Missing"],
);
assert.deepEqual(
  result.extra.map((row) => row.name),
  ["Extra"],
);
assert.equal(compareRows([row("same", [])], [row("same", [])]).matched.length, 1);
assert.equal(compareRows([], []).matched.length, 0);

// Distinct repeated waves must not all match the first simultaneous burst.
const waves = compareRows(
  [79.2, 81.2, 83.2, 85.2].map((time) => row("Spotlight", ["B49E"], time)),
  [79.709, 79.709, 79.709, 79.709, 81.718, 83.678, 85.639].map((time) =>
    row("Spotlight", ["B49E"], time),
  ),
);
assert.deepEqual(
  waves.matched.map((match) => match.generated.time),
  [79.709, 81.718, 83.678, 85.639],
);
const guarded = compareRows([row("A", ["A"], 10)], [row("A", ["A"], 112)], {
  clockOffset: 100,
  maxTimeDelta: 1,
});
assert.equal(guarded.matched.length, 0);
assert.equal(
  compareRows([row("A", ["A"], 10)], [row("A", ["A"], 111)], { clockOffset: 100, maxTimeDelta: 1 })
    .matched.length,
  1,
);
assert.throws(() => compareRows([], [], { maxTimeDelta: -1 }));
const tether = { ...row("A", ["0175"]), evidence: { type: "Tether" } };
const ability = { ...row("A", ["0175"]), evidence: { type: "Ability" } };
assert.equal(compareRows([tether], [ability]).matched.length, 0);
const simultaneous = simultaneousMatches([
  { reference: row("Explosion 4", ["A"], 10), generated: row("Explosion", ["A"], 11) },
  { reference: row("Explosion 5", ["A"], 10), generated: row("Explosion", ["A"], 11) },
  { reference: row("Other", ["B"], 10), generated: row("Other", ["B"], 11) },
]);
assert.equal(simultaneous.length, 1);
assert.equal(simultaneous[0].matches.length, 2);
const labels = suggestUnknownLabels([
  { reference: row("--center--", ["9FE5"], 18.3), generated: row("unknown_9fe5", ["9FE5"], 19.1) },
  {
    reference: row("--center--", ["9FE5"], 101.2),
    generated: row("unknown_9fe5", ["9FE5"], 101.9),
  },
  { reference: row("Other", ["BB"]), generated: row("unknown_aa", ["AA"]) },
]);
assert.equal(labels.length, 1);
assert.equal(labels[0].id, "9FE5");
assert.equal(labels[0].referenceName, "--center--");
assert.equal(labels[0].observations.length, 2);
assert.deepEqual(
  describeUnmatched([
    row("marker", []),
    { ...row("comment", ["A"]), evidence: { type: "Ability", disabled: true } },
    { ...row("tether", ["0175"]), evidence: { type: "Tether", disabled: false } },
  ]),
  {
    noNetworkEvidence: 1,
    activeEvidence: 1,
    commentedEvidence: 1,
    withoutIds: 1,
    byType: { Ability: 1, Tether: 1 },
  },
);

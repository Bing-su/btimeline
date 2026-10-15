// Test repeated mechanics and ID alternatives without requiring a game or network connection.
import assert from "node:assert/strict";
import { compareRows } from "./compare_cactbot.ts";
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

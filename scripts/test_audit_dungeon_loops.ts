// A matching opener is insufficient: preserve complete bodies, alternatives and source identity.
import assert from "node:assert/strict";
import { completeCycles, type Pattern, type Observation } from "./audit_dungeon_loops.ts";
const p = (id: string, time: number): Pattern => ({
  ids: id.split("|"),
  sources: ["Boss"],
  time,
  name: id,
});
const sample = (id: string, at: number, source = "Boss"): Observation => ({
  id,
  at,
  source,
  node: at,
});
const body = [p("A", 0), p("B|C", 1), p("D", 2)];
const rows = [
  sample("A", 0),
  sample("C", 1000),
  sample("D", 2000),
  sample("A", 3000),
  sample("B", 4000),
  sample("D", 5000),
  sample("A", 6000),
];
assert.equal(completeCycles(rows, body, p("A", 3)).length, 2);
assert.equal(
  completeCycles(
    rows.filter((row) => row.id !== "D"),
    body,
    p("A", 3),
  ).length,
  0,
);
assert.equal(
  completeCycles(
    rows.map((row) => ({ ...row, source: "Other" })),
    body,
    p("A", 3),
  ).length,
  0,
);
assert.equal(completeCycles(rows.slice(0, 3), body, p("A", 3)).length, 0);
assert.equal(
  completeCycles(
    rows.filter((row) => row.at !== 3000),
    body,
    p("A", 3),
  ).length,
  0,
);
const simultaneous = [rows[0], sample("B", 1000), sample("C", 1000), ...rows.slice(2, 4)];
assert.equal(completeCycles(simultaneous, body, p("A", 3)).length, 1);
assert.equal(completeCycles(simultaneous, body, p("A", 3))[0].body_nodes.length, 4);
assert.equal(
  completeCycles(
    rows.map((row) => ({ ...row, at: row.at * 10 })),
    body,
    p("A", 3),
  ).length,
  2,
);
assert.throws(() => completeCycles(rows, [], p("A", 3)));
assert.throws(() => completeCycles([...rows].reverse(), body, p("A", 3)));
assert.throws(() => completeCycles([sample("A", -1)], body, p("A", 3)));
console.log("dungeon loop audit tests passed");

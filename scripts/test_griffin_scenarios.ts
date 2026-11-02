import assert from "node:assert/strict";
import { griffinScenarios, type Sample } from "./griffin_scenarios.ts";

const samples: Sample[] = [
  [0, "B0EB", 1],
  [12000, "B0EC", 2],
  [82000, "B0EB", 3],
  [94000, "B0F3", 4],
  [95000, "B0F4", 5],
  [102000, "B0EC", 6],
  [136000, "B0F9", 7],
  [152000, "B0F5", 8],
];
const original = JSON.stringify(samples);
const scenarios = griffinScenarios(samples, 160000);
const omitted = scenarios.find((row) => row.name === "delayed-without-storm-or-volts")!;
assert(omitted.targets.some((row) => row[1] === "B0F3" && row[0] === 106000));
assert(!omitted.observations.some((row) => row[1] === "B0F3"));
assert(omitted.observations.some((row) => row[1] === "B0EC" && row[0] === 12000));
assert(!omitted.observations.some((row) => row[1] === "B0EC" && row[0] > omitted.onset_ms));
assert.equal(omitted.end_ms, 172000);
assert.equal(omitted.targets.at(-1)?.[2], 8);
assert.equal(omitted.event_ids[3], "rotation-storm-surge");
const roars = griffinScenarios(
  [
    [0, "B0EB", 1],
    [23000, "B0EF", 2],
    [82000, "B0EB", 3],
    [113000, "B0EF", 4],
    [124000, "B0EF", 5],
  ],
  140000,
)[0];
assert.equal(roars.event_ids[1], "opening-roar-first");
assert.equal(roars.event_ids[3], "rotation-roar-first");
assert.equal(roars.event_ids[4], "rotation-roar-second");
assert.equal(JSON.stringify(samples), original);
assert.equal(scenarios.find((row) => row.name === "early")?.onset_ms, 77000);
assert.throws(() => griffinScenarios([[0, "B0EB", 1]], 1000));
console.log("Griffin perturbation targets and observation isolation passed");

import assert from "node:assert/strict";

export type Sample = readonly [number, string, number];

const labels: Record<string, string> = {
  B0EB: "thunderspark",
  B0EC: "high-volts",
  B0F3: "storm-surge",
  B0F4: "electrifying-flight",
  B0F5: "fulgurous-fall",
  B0F8: "electrogenetic-force",
  B0F9: "golden-talons",
};

function eventIds(samples: readonly Sample[]) {
  let sparks = 0,
    roars = 0;
  return samples.map(([, id]) => {
    if (id === "B0EB") {
      sparks++;
      roars = 0;
    }
    const label = id === "B0EF" ? `roar-${["first", "second", "last"][roars++]}` : labels[id];
    assert(label && !label.includes("undefined"), `Unknown mechanic position: ${id}`);
    return sparks === 1
      ? `griffin/main/opening-${label}`
      : `griffin/main/loop/late-rotation/rotation-${label}`;
  });
}

// Perturb only the late rotation; omitting a signal removes observations, never the mechanic being evaluated.
export function griffinScenarios(samples: readonly Sample[], end_ms: number) {
  const onset = samples.findIndex((sample, index) => index > 0 && sample[1] === "B0EB");
  assert(onset > 0, "A second Thunderspark is required for the late-rotation scenarios");
  const event_ids = eventIds(samples);
  return [
    { name: "early", shift_ms: -5000, omit: [] },
    { name: "delayed", shift_ms: 12000, omit: [] },
    { name: "early-without-storm", shift_ms: -5000, omit: ["B0F3"] },
    { name: "delayed-without-storm", shift_ms: 12000, omit: ["B0F3"] },
    { name: "delayed-without-storm-or-volts", shift_ms: 12000, omit: ["B0F3", "B0EC"] },
    { name: "delayed-without-anchors", shift_ms: 12000, omit: ["B0EB", "B0F3", "B0EC", "B0F9"] },
  ].map((spec) => {
    const targets: Sample[] = samples.map(([at, id, index], position) => [
      at + (position >= onset ? spec.shift_ms : 0),
      id,
      index,
    ]);
    assert(
      targets.every((sample, index) => index === 0 || sample[0] >= targets[index - 1][0]),
      "Perturbation reordered mechanics",
    );
    return {
      ...spec,
      end_ms: end_ms + spec.shift_ms,
      onset_ms: targets[onset][0],
      targets,
      event_ids,
      observations: targets.filter(
        (sample, position) => position < onset || !spec.omit.includes(sample[1]),
      ),
    };
  });
}

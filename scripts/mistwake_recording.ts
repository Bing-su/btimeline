import { InputExclusion } from "./verify_griffin_holdout.ts";

export const mistwakeBosses = [
  {
    phase: "treno",
    source: "Treno Catoblepas",
    entry: ["A93F"],
    names: {
      A93F: "Earthquake",
      A943: "Thunder II",
      A942: "Bedeviling Light",
      A941: "Thunder III",
      AF19: "Ray of Lightning",
      A947: "Petribreath",
    },
  },
  {
    phase: "amdusias",
    source: "Amdusias",
    entry: ["B118", "B11D"],
    names: {
      B118: "Thunderclap Concerto",
      B11D: "Thunderclap Concerto",
      B120: "Bio II",
      B122: "Galloping Thunder",
      B126: "Thunder IV",
      B12B: "Shockbolt",
      B11F: "Thunder",
      B129: "Thunder III",
    },
  },
  {
    phase: "griffin",
    source: "Thundergust Griffin",
    entry: ["B0EB"],
    names: {
      B0EB: "Thunderspark",
      B0EC: "High Volts",
      B0EF: "Thundering Roar",
      B0F3: "Storm Surge",
      B0F4: "Electrifying Flight",
      B0F5: "Fulgurous Fall",
      B0F8: "Electrogenetic Force",
      B0F9: "Golden Talons",
    },
  },
] as const;

// Preserve every cast as an observation; collapse only duplicate target packets in the mechanic targets.
export function mistwakeRecording(raw: any) {
  const actors = new Map<number, string>(
    raw.report.masterData.actors.map((actor: any) => [actor.id, actor.name]),
  );
  const origin = raw.events[0].timestamp;
  const observations = raw.events.flatMap((event: any, raw_index: number) => {
    if (event.type !== "cast") return [];
    const source = actors.get(event.sourceID);
    if (!source)
      throw new InputExclusion("Unknown cast source", { raw_index, source_id: event.sourceID });
    return [
      {
        at_ms: event.timestamp - origin,
        type: "Ability",
        fields: {
          source,
          id: event.abilityGameID.toString(16).toUpperCase(),
        },
        raw_index,
      },
    ];
  });
  const bosses = mistwakeBosses.map((boss) => {
    const entry = observations.find(
      (item: any) =>
        item.fields.source === boss.source &&
        (boss.entry as readonly string[]).includes(item.fields.id),
    );
    if (!entry) throw new InputExclusion("Missing boss entry", { source: boss.source });
    const completion = raw.events.findIndex(
      (event: any) =>
        event.timestamp >= origin + entry.at_ms &&
        event.type === "damage" &&
        actors.get(event.targetID) === boss.source &&
        event.targetResources?.hitPoints === 0,
    );
    if (completion < 0)
      throw new InputExclusion("Missing observed boss completion", {
        source: boss.source,
        entry_event: entry.raw_index,
      });
    const end_ms = raw.events[completion].timestamp - origin;
    const names: Record<string, string> = boss.names;
    const last = new Map<string, number>();
    const targets = observations.filter((item: any) => {
      const id = item.fields.id;
      if (
        item.at_ms < entry.at_ms ||
        item.at_ms > end_ms ||
        !names[id] ||
        item.fields.source !== (id === "B0F4" ? "Shock Storm" : boss.source)
      )
        return false;
      // Amdusias Thunder emits one packet per target; keep one display target per 100ms wave.
      if (item.at_ms - (last.get(id) ?? -Infinity) <= 100) return false;
      last.set(id, item.at_ms);
      return true;
    });
    return {
      phase: boss.phase,
      source: boss.source,
      entry_ms: entry.at_ms,
      end_ms,
      entry_event: entry.raw_index,
      completion_event: completion,
      targets: targets.map((item: any) => ({ ...item, name: names[item.fields.id] })),
    };
  });
  if (bosses.some((boss, index) => index > 0 && boss.entry_ms <= bosses[index - 1]!.end_ms))
    throw new InputExclusion("Overlapping or out-of-order bosses", {});
  return { origin_timestamp_ms: origin, observations, bosses };
}

import { InputExclusion } from "./verify_griffin_holdout.ts";

export const clyteumBosses = [
  {
    phase: "eye",
    source: "Eye of the Scorpion",
    entry: "BF00",
    names: {
      BF00: "Eyes on Me",
      C3FF: "Petrifying Beam",
      C400: "Petrifying Beam",
      BEFD: "Motion Scanner",
      BF01: "Ballistic Missile",
      BF05: "Penetrator Missile",
      BF02: "Surface Missile",
      BF03: "Anti-Personnel Missile",
    },
  },
  {
    phase: "chort",
    source: "Chort",
    entry: "BEF4",
    names: {
      BEF4: "Ripples of Gloom",
      BEE4: "Mortifying Flesh",
      BEE5: "Mortifying Flesh",
      BEE6: "Mortifying Flesh",
      BEE7: "Mortifying Flesh",
      BEEC: "Mortifying Flesh",
      BEEE: "Bodyweight Exorcism",
      C4B9: "Basic Vomit",
      BEEF: "Bodyweight Exorcism",
      C4F1: "Evil Emission",
      BEF6: "Profane Pressure",
    },
  },
  {
    phase: "malphas",
    source: "Malphas",
    entry: "BF18",
    names: {
      BF18: "Rubbish Disposal",
      C489: "Void Dark",
      BF19: "Scrap Meddle",
      BF1A: "Puppet Strings",
      BF21: "Gluttonous Wire",
      BF1B: "Puppet Pair",
      BF23: "String Up",
      C48B: "Shadow Play",
      BF1D: "Puppet Mastery",
    },
  },
] as const;

// Require one clean completed attempt per boss: a dungeon log can otherwise concatenate wipes into a false rotation.
export function clyteumRecording(raw: any) {
  const actors = new Map<number, any>(
    raw.report.masterData.actors.map((actor: any) => [actor.id, actor]),
  );
  const origin = raw.events[0].timestamp;
  const bosses = clyteumBosses.map((boss) => {
    const main = raw.report.masterData.actors.filter(
      (actor: any) => actor.name === boss.source && actor.subType === "Boss",
    );
    if (main.length !== 1)
      throw new InputExclusion("Unclear main boss actor", { source: boss.source });
    const entry = raw.events.findIndex(
      (event: any) =>
        event.type === "cast" &&
        actors.get(event.sourceID)?.name === boss.source &&
        event.abilityGameID.toString(16).toUpperCase() === boss.entry,
    );
    if (entry < 0) throw new InputExclusion("Missing boss entry", { source: boss.source });
    const start = raw.events[entry].timestamp;
    const completion = raw.events.findIndex(
      (event: any) =>
        event.timestamp >= start &&
        event.type === "damage" &&
        event.targetID === main[0].id &&
        event.targetResources?.hitPoints === 0,
    );
    if (completion < 0)
      throw new InputExclusion("Missing observed boss completion", {
        source: boss.source,
        entry_event: entry,
      });
    const end = raw.events[completion].timestamp;
    const health = raw.events.flatMap((event: any, raw_index: number) =>
      event.timestamp >= start &&
      event.timestamp <= end &&
      event.type === "damage" &&
      event.targetID === main[0].id &&
      event.targetResources?.maxHitPoints > 0
        ? [
            {
              ratio: event.targetResources.hitPoints / event.targetResources.maxHitPoints,
              raw_index,
            },
          ]
        : [],
    );
    let lowest = 1;
    for (const sample of health) {
      if (lowest < 0.75 && sample.ratio > 0.9)
        throw new InputExclusion("Possible attempt reset or large boss heal", {
          source: boss.source,
          raw_index: sample.raw_index,
        });
      lowest = Math.min(lowest, sample.ratio);
    }
    const last = new Map<string, number>();
    const names: Record<string, string> = boss.names;
    const targets = raw.events.flatMap((event: any, raw_index: number) => {
      if (
        event.timestamp < start ||
        event.timestamp > end ||
        event.type !== "cast" ||
        actors.get(event.sourceID)?.name !== boss.source
      )
        return [];
      const id = event.abilityGameID.toString(16).toUpperCase();
      if (!names[id]) return [];
      const key =
        id === "C3FF" || id === "C400"
          ? "beam"
          : ["BEE4", "BEE5", "BEE6", "BEE7"].includes(id)
            ? "mortifying"
            : id;
      if (event.timestamp - (last.get(key) ?? -Infinity) <= 100) return [];
      last.set(key, event.timestamp);
      return [
        {
          raw_index,
          at_ms: event.timestamp - origin,
          fields: { id, source: boss.source },
          name: names[id],
        },
      ];
    });
    return {
      phase: boss.phase,
      source: boss.source,
      entry_ms: start - origin,
      end_ms: end - origin,
      entry_event: entry,
      completion_event: completion,
      targets,
    };
  });
  if (bosses.some((boss, index) => index > 0 && boss.entry_ms <= bosses[index - 1]!.end_ms))
    throw new InputExclusion("Overlapping boss attempts", {});
  const observations = raw.events.flatMap((event: any, raw_index: number) => {
    const at_ms = event.timestamp - origin;
    if (
      event.type !== "cast" ||
      !bosses.some((boss) => at_ms >= boss.entry_ms && at_ms <= boss.end_ms)
    )
      return [];
    const source = actors.get(event.sourceID)?.name;
    if (!source)
      throw new InputExclusion("Unknown cast source in boss scope", {
        raw_index,
        source_id: event.sourceID,
      });
    return [
      {
        at_ms,
        type: "Ability",
        fields: { source, id: event.abilityGameID.toString(16).toUpperCase() },
      },
    ];
  });
  return { origin_timestamp_ms: origin, bosses, observations };
}

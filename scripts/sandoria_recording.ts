import { InputExclusion } from "./verify_griffin_holdout.ts";
export const sandoriaBosses = [
  {
    phase: "kirin",
    source: "Faithbound Kirin",
    entry: "ADCA",
    names: {
      ADCA: "Stonega IV",
      AD91: "Wrought Arms",
      AD95: "Synchronized Strike",
      AFF4: "Crimson Riddle",
      AFF5: "Crimson Riddle",
      AD7E: "Summon Shijin",
      B07B: "Quake",
      B07C: "Quake",
      AD92: "Wringer",
      AD94: "Striking Right / Striking Left",
      ADAD: "Wringer / Synchronized Strike",
      ADB0: "Wringer / Synchronized Strike",
      AD9D: "Synchronized Sequence / Double Wringer",
      ADA5: "Wringer / Synchronized Strike",
      ADB1: "Mighty Grip",
      ADB2: "Deadly Hold",
      ADBF: "Bury",
      B079: "Stonega III",
      B07A: "Stonega III",
      AD93: "Striking Right / Striking Left",
      ADA0: "Synchronized Sequence / Double Wringer",
      ADA2: "Wringer / Synchronized Strike",
    },
  },
  {
    phase: "duo",
    source: "Omega, the One",
    entry: "AD2B",
    other: "Ultima, the Feared",
    names: {
      AD2B: "Omega: Ion Efflux",
      AD27: "Omega: Fore-to-Aft Fire / Aft-to-Fore Fire",
      B087: "Omega: Anti-Personnel Missile",
      B075: "Omega: Surface Missile",
      AD23: "Omega: Trajectory Projection",
      AD24: "Omega: Guided Missile",
      AFEB: "Omega: Multi-Missile",
      AD2F: "Omega: Hyper Pulse",
      AD25: "Omega: Fore-to-Aft Fire / Aft-to-Fore Fire",
      AD10: "Ultima: Antimatter",
      AD08: "Ultima: Energy Orb",
      AD06: "Ultima: Tractor Beam",
      AD09: "Ultima: Mana Screen",
      AD0D: "Ultima: Energy Ray",
      AD12: "Ultima: Tractor Field",
      AD14: "Ultima: Citadel Siege",
      AD1B: "Ultima: Citadel Buster",
      AD0C: "Ultima: Energy Ray",
      AD0E: "Ultima: Chemical Bomb",
    },
  },
  {
    phase: "kam",
    source: "Kam'lanaut",
    entry: "ACBD",
    names: {
      ACBD: "Enspirited Swordplay",
      B009: "Proving Ground",
      AC91: "Elemental Blade",
      B00A: "Sublime Elements",
      ACBB: "Princely Blow",
      ACAB: "Light Blade",
      ACAF: "Great Wheel",
      ACB1: "Great Wheel",
      ACB2: "Esoteric Scrivening",
      ACB3: "Shockwave",
      ACB4: "Transcendent Union",
      ACB5: "Transcendent Union",
      ACB6: "Esoteric Palisade",
      ACB7: "Crystalline Resonance",
      ACB8: "Elemental Resonance",
      ACB9: "Illumed Facet",
      ACBF: "Empyreal Banish III",
      ACBA: "Illumed Estoc",
      ACBE: "Shield Bash",
      AF6B: "Empyreal Banish IV",
      AC92: "Elemental Blade",
      ACAE: "Great Wheel",
      ACB0: "Great Wheel",
      ACAD: "Great Wheel",
    },
  },
  {
    phase: "eald",
    source: "Eald'narche",
    entry: "AD51",
    names: {
      AD51: "Uranos Cascade",
      AD49: "Cronos Sling",
      AD6D: "Empyreal Vortex",
      AD4B: "Cronos Sling",
      AD56: "Warp",
      AD58: "Sleepga",
      AD53: "Gaea Stream",
      AD5C: "Omega Javelin",
      AD77: "Duplicate",
      B040: "Excelsior",
      AD59: "Phase Shift",
      AD5A: "Phase Shift",
      AD75: "Visions of Paradise",
      AD76: "Duplicate",
      AD72: "Stellar Burst",
      AD5F: "Ancient Triad",
      AD4A: "Cronos Sling",
    },
  },
];

// Each FFLogs fight is one encounter; keep the two simultaneous bosses in the same segment.
export function sandoriaRecording(raw: any) {
  const encounter = raw.report.fights[0].encounterID;
  const boss = sandoriaBosses[encounter - 2067];
  if (!boss) throw new InputExclusion("Unsupported Sandoria encounter", { encounter });
  const actors = new Map<number, any>(raw.report.masterData.actors.map((a: any) => [a.id, a]));
  const sources = [boss.source, ...(boss.other ? [boss.other] : [])];
  const main = sources.map((source) => {
    const matches = raw.report.masterData.actors.filter(
      (a: any) => a.name === source && a.subType === "Boss",
    );
    if (matches.length !== 1) throw new InputExclusion("Unclear main boss actor", { source });
    return matches[0];
  });
  const idOf = (event: any) => event.abilityGameID?.toString(16).toUpperCase();
  const entry = raw.events.findIndex(
    (event: any) =>
      event.type === "cast" &&
      actors.get(event.sourceID)?.name === boss.source &&
      idOf(event) === boss.entry,
  );
  if (entry < 0) throw new InputExclusion("Missing boss entry", { source: boss.source });
  const origin = raw.events[0].timestamp;
  const start = raw.events[entry].timestamp;
  // A wipe is a censored attempt, not a missing completion signal; do not fabricate an ACT finish from HP.
  // Evaluate only the authored finite route, even when a later holdout survives beyond the development observations.
  const limits: Record<string, number> = { kirin: 230000, duo: 238000, kam: 325000, eald: 357000 };
  const end = Math.min(raw.report.fights[0].endTime, start + limits[boss.phase]!);
  if (!(end > start)) throw new InputExclusion("Empty encounter scope", { entry });
  for (const actor of main) {
    let lowest = 1;
    for (const [raw_index, event] of raw.events.entries()) {
      if (
        event.timestamp < start ||
        event.timestamp > end ||
        event.type !== "damage" ||
        event.targetID !== actor.id ||
        !(event.targetResources?.maxHitPoints > 0)
      )
        continue;
      const ratio = event.targetResources.hitPoints / event.targetResources.maxHitPoints;
      if (lowest < 0.75 && ratio > 0.9)
        throw new InputExclusion("Possible attempt reset or large boss heal", {
          source: actor.name,
          raw_index,
        });
      lowest = Math.min(lowest, ratio);
    }
  }
  const names: Record<string, string> = boss.names;
  const last = new Map<string, number>();
  const targets = raw.events.flatMap((event: any, raw_index: number) => {
    if (
      event.timestamp < start ||
      event.timestamp > end ||
      event.type !== "cast" ||
      !sources.includes(actors.get(event.sourceID)?.name)
    )
      return [];
    const id = idOf(event);
    if (!names[id]) return [];
    const name = names[id];
    if (event.timestamp - (last.get(name) ?? -Infinity) <= 100) return [];
    last.set(name, event.timestamp);
    return [
      {
        raw_index,
        at_ms: event.timestamp - origin,
        fields: { id },
        name,
        // These choices become observable only ~3 seconds before resolution; evaluate after selection, before impact.
        ...(["B079", "B07B"].includes(id) ? { lead_ms: 2000 } : {}),
      },
    ];
  });
  const observations = raw.events.flatMap((event: any, raw_index: number) => {
    if (
      event.timestamp < start ||
      event.timestamp > end ||
      !["cast", "begincast"].includes(event.type)
    )
      return [];
    const source = actors.get(event.sourceID)?.name;
    if (!source)
      throw new InputExclusion("Unknown cast source in encounter scope", {
        raw_index,
        source_id: event.sourceID,
      });
    return [
      {
        at_ms: event.timestamp - origin,
        type: event.type === "cast" ? "Ability" : "StartsUsing",
        fields: { source, id: idOf(event) },
      },
    ];
  });
  return {
    origin_timestamp_ms: origin,
    observations,
    bosses: [
      {
        phase: boss.phase,
        entry_ms: start - origin,
        end_ms: end - origin,
        entry_event: entry,
        completion_event: raw.events.findLastIndex((e: any) => e.timestamp <= end),
        targets,
      },
    ],
  };
}

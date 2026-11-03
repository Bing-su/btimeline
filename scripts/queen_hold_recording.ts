import { InputExclusion } from "./types.ts";

const mechanics: Record<string, [string, string]> = {
  "8F00": ["shift", "Virtual Shift"],
  "8F15": ["coronation", "Coronation"],
  "8EFC": ["aethertithe", "Aethertithe"],
  "8F31": ["aethertithe", "Aethertithe"],
  "8F32": ["aethertithe", "Aethertithe"],
  "8F33": ["aethertithe", "Aethertithe"],
  "8F1A": ["regalia", "Ruthless Regalia"],
  "8EFA": ["prosecution", "Prosecution of War"],
  "8F1E": ["force-first", "Legitimate Force"],
  "8F1F": ["force-first", "Legitimate Force"],
  "8F20": ["force-first", "Legitimate Force"],
  "8F21": ["force-first", "Legitimate Force"],
  "8F22": ["force-second", "Legitimate Force"],
  "8F23": ["force-second", "Legitimate Force"],
  "8EFB": ["royal", "Royal Domain"],
  "8EFF": ["exit-shift", "Virtual Shift"],
  "8F05": ["castellation", "Castellation"],
  "8F06": ["besiegement", "Besiegement"],
  "8F07": ["besiegement", "Besiegement"],
  "8F08": ["besiegement", "Besiegement"],
  "8F09": ["besiegement", "Besiegement"],
  "8F0A": ["besiegement", "Besiegement"],
};

// Scope the first Castellation hold only; the later return to 8F00 is an endpoint, not a new start.
export function queenHoldRecording(raw: any) {
  const actors = new Map<number, string>(
    raw.report.masterData.actors.map((actor: any) => [actor.id, actor.name]),
  );
  const queen = (event: any) => actors.get(event.sourceID) === "Queen Eternal";
  const entry = raw.events.findIndex(
    (event: any) => event.type === "cast" && event.abilityGameID === 0x8f00 && queen(event),
  );
  if (entry < 0) throw new InputExclusion("Missing hold approach", {});
  const origin = raw.events[entry].timestamp;
  const exit = raw.events.findIndex(
    (event: any) =>
      event.timestamp > origin &&
      event.type === "begincast" &&
      event.abilityGameID === 0x8eff &&
      queen(event),
  );
  if (exit < 0) throw new InputExclusion("Missing observed hold exit", { entry_event: entry });
  const exit_ms = raw.events[exit].timestamp - origin;
  const end = raw.events.findIndex(
    (event: any) =>
      event.timestamp > origin + exit_ms &&
      event.type === "cast" &&
      event.abilityGameID === 0x8f00 &&
      queen(event),
  );
  if (end < 0) throw new InputExclusion("Missing post-exit continuation", { exit_event: exit });
  const end_ms = raw.events[end].timestamp - origin;
  const observations = raw.events.flatMap((event: any, raw_index: number) => {
    if (
      event.timestamp < origin ||
      event.timestamp > origin + end_ms ||
      !["cast", "begincast"].includes(event.type)
    )
      return [];
    const source = actors.get(event.sourceID);
    if (!source)
      throw new InputExclusion("Unknown cast source", { raw_index, source_id: event.sourceID });
    return [
      {
        at_ms: event.timestamp - origin,
        type: event.type === "cast" ? "Ability" : "StartsUsing",
        fields: { source, id: event.abilityGameID.toString(16).toUpperCase() },
        raw_index,
      },
    ];
  });
  const last = new Map<string, number>();
  const targets = observations
    .filter((item: any) => {
      const mechanic = mechanics[item.fields.id];
      if (!mechanic || item.type !== "Ability" || item.fields.source !== "Queen Eternal")
        return false;
      const [key] = mechanic;
      // Different packet IDs can describe one directional cast; preserve the observations and collapse only display targets.
      if (item.at_ms - (last.get(key) ?? -Infinity) <= 100) return false;
      last.set(key, item.at_ms);
      return true;
    })
    .map((item: any) => {
      const [kind, name] = mechanics[item.fields.id]!;
      const stage =
        item.at_ms >= exit_ms
          ? "after-exit"
          : ["prosecution", "force-first", "force-second", "royal"].includes(kind)
            ? "hold"
            : "approach";
      return {
        ...item,
        kind,
        name,
        stage,
        lead_ms: item.at_ms === 0 ? null : kind === "exit-shift" ? 3000 : 5000,
      };
    });
  const openings = targets.filter(
    (item: any) => item.stage === "hold" && item.kind === "prosecution",
  );
  const body = [
    "prosecution",
    "force-first",
    "force-second",
    "royal",
    "force-first",
    "force-second",
  ];
  const complete_bodies = openings.filter((opening: any, index: number) => {
    const finish = openings[index + 1]?.at_ms ?? exit_ms;
    return (
      JSON.stringify(
        targets
          .filter((item: any) => item.at_ms >= opening.at_ms && item.at_ms < finish)
          .map((item: any) => item.kind),
      ) === JSON.stringify(body)
    );
  }).length;
  return {
    origin_timestamp_ms: origin,
    entry_event: entry,
    exit_event: exit,
    end_event: end,
    exit_ms,
    end_ms,
    observations,
    targets,
    hold_openings: openings.map((item: any) => [item.at_ms, item.raw_index]),
    complete_bodies,
    observed_recurrences: Math.max(0, openings.length - 1),
  };
}

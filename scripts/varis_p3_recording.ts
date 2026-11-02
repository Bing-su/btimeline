import { InputExclusion } from "./verify_griffin_holdout.ts";

export const varisBlocks = {
  L: {
    id: "4CD8",
    names: {
      "4CD8": "Loaded Gunshield",
      "4CDF": "Magitek Burst",
      "4CB6": "Ignis Est",
      "4CB4": "Terminus Est",
      "4CC9": "Festina Lente",
      "4CF0": "Citius",
    },
  },
  R: {
    id: "4CD9",
    names: {
      "4CD9": "Reinforced Gunshield",
      "4CCA": "Altius",
      "4CB4": "Terminus Est",
      "4CDB": "Magitek Shielding",
      "4CDC": "Magitek Shielding",
      "4CC7": "Ventus Est",
      "4CF0": "Citius",
    },
  },
  E: {
    id: "4CD7",
    names: {
      "4CD7": "Electrified Gunshield",
      "4CEA": "Reinforcements",
      "4CED": "Aetherochemical Grenado",
      "4CB4": "Terminus Est",
      "4CDA": "Magitek Shock",
      "4CD2": "Alea Iacta Est",
    },
  },
} as const;
export type VarisBlock = keyof typeof varisBlocks;

// The pinned ACT reference capitalizes these NPC names differently from FFLogs; keep this projection explicit.
const canonical = (name: string) =>
  ({ "varis yae galvus": "Varis Yae Galvus", bladesblood: "Bladesblood", gunshield: "Gunshield" })[
    name.toLowerCase()
  ] ?? name;
const sourceFor = (id: string) =>
  ({
    "4CB6": "Ignis Est",
    "4CC7": "Ventus Est",
    "4CB4": "Terminus Est",
    "4CED": "Magitek Turret II",
    "4CE3": "Gunshield",
    "4CE4": "Gunshield",
  })[id] ?? "Varis Yae Galvus";

export function varisP3Recording(raw: any) {
  const actors = new Map<number, string>(
    raw.report.masterData.actors.map((actor: any) => [actor.id, actor.name]),
  );
  const named = (event: any, source: string) => {
    const name = actors.get(event.sourceID);
    return name ? canonical(name) === source : false;
  };
  const entry = raw.events.findIndex(
    (event: any) =>
      event.type === "cast" && event.abilityGameID === 0x4ccf && named(event, "Bladesblood"),
  );
  if (entry < 0) throw new InputExclusion("Missing P3 entry", {});
  const origin = raw.events[entry].timestamp;
  const join = raw.events.findIndex(
    (event: any) =>
      event.timestamp > origin &&
      event.type === "cast" &&
      event.abilityGameID === 0x4ce0 &&
      named(event, "Varis Yae Galvus"),
  );
  if (join < 0) throw new InputExclusion("Missing P3 shared continuation", { entry_event: entry });
  const join_ms = raw.events[join].timestamp - origin;
  const end_ms = Math.min(join_ms + 22000, raw.report.fights[0].endTime - origin);
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
        fields: { source: canonical(source), id: event.abilityGameID.toString(16).toUpperCase() },
        raw_index,
      },
    ];
  });
  const choices = observations.filter(
    (item: any) =>
      item.type === "StartsUsing" &&
      item.at_ms < join_ms &&
      item.fields.source === "Varis Yae Galvus" &&
      Object.values(varisBlocks).some((block) => block.id === item.fields.id),
  );
  const order = choices.map(
    (item: any) =>
      Object.entries(varisBlocks).find(
        ([, block]) => block.id === item.fields.id,
      )![0] as VarisBlock,
  );
  if (order.length !== 3 || new Set(order).size !== 3)
    throw new InputExclusion("P3 needs three distinct choices", { order });
  const blocks = choices.map((choice: any, position: number) => {
    const letter = order[position]!;
    const names: Record<string, string> = varisBlocks[letter].names;
    const end = choices[position + 1]?.at_ms ?? join_ms;
    const last = new Map<string, number>();
    const targets = observations
      .filter((item: any) => {
        const id = item.fields.id;
        if (
          item.type !== "Ability" ||
          item.at_ms < choice.at_ms ||
          item.at_ms >= end ||
          !names[id] ||
          item.fields.source !== sourceFor(id)
        )
          return false;
        if (item.at_ms - (last.get(id) ?? -Infinity) <= 100) return false;
        last.set(id, item.at_ms);
        return true;
      })
      .map((item: any) => ({
        ...item,
        name: names[item.fields.id],
        lead_ms: item.fields.id === varisBlocks[letter].id ? 3000 : 5000,
      }));
    return {
      letter,
      phase: position === 2 ? `final-${letter}` : order.slice(0, position + 1).join(""),
      entry_ms: choice.at_ms,
      entry_event: choice.raw_index,
      end_ms: end,
      targets,
    };
  });
  const shared = observations
    .filter(
      (item: any) =>
        item.type === "Ability" &&
        ["4CE3", "4CE4"].includes(item.fields.id) &&
        item.fields.source === "Gunshield",
    )
    .filter(
      (item: any, index: number, items: any[]) =>
        index === 0 || item.at_ms - items[index - 1].at_ms > 100,
    )
    .map((item: any) => ({ ...item, name: "Magitek Spark / Magitek Torch", lead_ms: 5000 }));
  return {
    origin_timestamp_ms: origin,
    entry_event: entry,
    join_event: join,
    join_ms,
    end_ms,
    observations,
    order: order.join(""),
    blocks: [
      ...blocks,
      {
        letter: "joined",
        phase: "joined",
        entry_ms: join_ms,
        entry_event: join,
        end_ms,
        targets: shared,
      },
    ],
  };
}

// Describe the fields these audits consume, e.g. replay rows; runtime assertions still validate evidence.
export type CollectedLog = {
  report: {
    masterData: {
      actors: { id: number; name: string; subType?: string }[];
      abilities: { gameID: number; name: string }[];
    };
    fights: {
      startTime: number;
      enemyNPCs: { id: number }[];
      enemyPets?: { id: number }[];
      enemyPlayers?: number[];
    }[];
  };
  events: {
    type: string;
    timestamp: number;
    sourceID?: number;
    targetID?: number;
    abilityGameID?: number;
    melee?: boolean;
    sealType?: number;
  }[];
};
type CoverageInput = {
  input?: { file: string };
  bossSegments?: { startMs: number; endMs: number }[];
};
type OutputCoverage = {
  file: string;
  representedEventIndices: number[];
  excludedEventIndices?: number[];
  omittedEventIndices?: number[];
};
export type GenerationReport = CoverageInput & {
  mode: string;
  slots?: {
    selector?: unknown;
    predictive?: boolean;
    name?: string;
    burstOffsetsMs?: number[];
    samples?: { file: string; eventIndices: number[] }[];
  }[];
  inputs?: CoverageInput[];
  segments?: { report: CoverageInput & { inputs?: CoverageInput[] } }[];
  outputCoverage?: OutputCoverage[];
  emittedEventIndices?: number[];
  collapsedCasts?: { representativeEventIndex: number; omittedEventIndices: number[] }[];
  schedule?: { segments: { file: string; spans: { reset: number | null }[] }[] };
};
export type ReplayCoverageReport = {
  pulls: {
    file: string;
    excludedFromGeneration?: boolean;
    unrepresentedEventIndices: number[];
    excludedEventIndices: number[];
  }[];
};
export type ReplayReport = {
  pulls: (ReplayCoverageReport["pulls"][number] & {
    report: string;
    fight: number;
    termination: string;
    replay: {
      previews: { moment: string; sourceMs: number }[];
      rows: {
        entryIndex: number;
        name: string;
        packetEventIndices?: number[];
        expectedEventIndices: number[];
        matches: { eventIndex: number }[];
      }[];
    };
  })[];
};
export type EvaluationReport = {
  binarySha256: string;
  manifestSha256: string;
  groups: {
    evaluation: string;
    group: { encounter: number; difficulty: number };
    status: string;
    directory?: string;
    holdout?: unknown;
  }[];
};
export type TimelineDocument = {
  resetOn?: string[];
  entries: { kind: string; sync?: { log: string } }[];
};
export type ConsumerSync = {
  time: number;
  lineNumber: number;
  start: number;
  end: number;
  jump?: number;
  jumpType?: string;
  regexType: string;
  origInput?: Record<string, unknown>;
  regex: RegExp;
};
export type ConsumerEvent = {
  id: string;
  name: string;
  time: number;
  lineNumber: number;
  duration?: number;
  sync?: ConsumerSync;
};
export type ParsedTimeline = {
  events: ConsumerEvent[];
  errors: unknown[];
  ignores: Record<string, unknown>;
  forceJumps: { time: number; jump?: number; jumpType?: string }[];
  syncStarts: unknown[];
};
export type ConsumerTimeline = {
  events: ConsumerEvent[];
  activeNetSyncs: ConsumerSync[];
  timebase: number;
  OnLogLineJump: (sync: ConsumerSync, time: number) => void;
  SyncTo: (time: number, now: number) => void;
};

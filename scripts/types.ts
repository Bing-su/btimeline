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

// Retained by the legacy Varis and Queen projectors until their evaluation moves to Rust.
export class InputExclusion extends Error {
  constructor(
    message: string,
    readonly evidence: Record<string, unknown>,
  ) {
    super(message);
    this.name = "InputExclusion";
  }
}

export function holdoutSummary(traces: { status: string; passed: boolean | null }[]) {
  const eligible = traces.filter((trace) => trace.status !== "excluded");
  return {
    selected: traces.length,
    excluded: traces.length - eligible.length,
    eligible: eligible.length,
    passed_traces: eligible.filter((trace) => trace.passed === true).length,
    passed: eligible.length > 0 && eligible.every((trace) => trace.passed === true),
  };
}

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

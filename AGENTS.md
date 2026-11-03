# Repository instructions

## Contracts

| When changing                                            | Read                                               |
| -------------------------------------------------------- | -------------------------------------------------- |
| Timeline syntax, validation, or execution semantics      | [Timeline schema](docs/TIMELINE_SCHEMA.md)         |
| FFLogs-to-KDL generation or mode policies                | [Generation contract](docs/GENERATION.md)          |
| Replay input, supported behavior, or verification claims | [Offline replay guide](docs/OFFLINE_REPLAY.md)     |
| Authored evaluation, evidence audit, or route coverage   | [Authored evaluation](docs/AUTHORED_EVALUATION.md) |
| FFLogs collection, mining, generation, or project goals  | [Implementation plan](docs/PLAN.md)                |

Final KDL must run in an external executor without evidence or FFLogs-only signals. Keep observations and uncertainty traceable, and report what replay could not verify.

For encounter timeline authoring or mechanic interpretation, use the repository [timeline-author skill](.agents/skills/timeline-author/SKILL.md). Keep agent interpretation and tool capabilities within the [authoring agreement](docs/AGENT_TIMELINE_AUTHORING.md).

## Timeline quality

Build useful mechanic forecasts with safe control and recovery at later syncs. Use cactbot's practical authoring approach as a reference; exact log reconstruction is a diagnostic, not the acceptance goal. For generation, validation, or quality reporting, read [Practical quality criteria](docs/GENERATION.md#실용적인-품질-기준).

- Preserve raw observations separately from display summaries. Group near-simultaneous mechanics or label alternatives when that preserves useful forecasts.
- Distinguish display uncertainty from sync, branch, and loop control safety. Retain useful displays while restricting unsafe control to the affected region.
- Explain whether `unresolved` means an implementation limit, insufficient evidence, or an observed contradiction; assess its user-visible impact separately.
- Verify advance warning, control safety, and recovery with standalone KDL. Keep schema validation, input integrity, and training/validation separation strict.

## Preferences

- Use `ast-grep` for structural code operations, `rg` for text search, and `sd` for straightforward replacements.
- Prefer functional programming: pure transformations, immutable values, and explicit I/O boundaries. Keep necessary mutation local.
- Prefer tables for rules and comparisons. Explain intent in comments and include examples when useful.

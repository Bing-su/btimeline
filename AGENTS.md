# Repository instructions

## Contracts

| When changing                                            | Read                                           |
| -------------------------------------------------------- | ---------------------------------------------- |
| Timeline syntax, validation, or execution semantics      | [Timeline schema](docs/TIMELINE_SCHEMA.md)     |
| Replay input, supported behavior, or verification claims | [Offline replay guide](docs/OFFLINE_REPLAY.md) |
| FFLogs collection, mining, generation, or project goals  | [Implementation plan](docs/PLAN.md)            |

Final KDL must run in an external executor without evidence or FFLogs-only signals. Keep observations and uncertainty traceable, and report what replay could not verify.

## Preferences

- Use `ast-grep` for structural code operations, `rg` for text search, and `sd` for straightforward replacements.
- Prefer functional programming: pure transformations, immutable values, and explicit I/O boundaries. Keep necessary mutation local.
- Prefer tables for rules and comparisons. Explain intent in comments and include examples when useful.

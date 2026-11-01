# Repository instructions

## Contracts

| When changing                                            | Read                                           |
| -------------------------------------------------------- | ---------------------------------------------- |
| Timeline syntax, validation, or execution semantics      | [Timeline schema](docs/TIMELINE_SCHEMA.md)     |
| Replay input, supported behavior, or verification claims | [Offline replay guide](docs/OFFLINE_REPLAY.md) |

## Preferences

- Use `ast-grep` for structural code operations, `rg` for text search, and `sd` for straightforward replacements.
- Prefer functional programming: pure transformations, immutable values, and explicit I/O boundaries. Keep necessary mutation local.
- Prefer tables for rules and comparisons. Explain intent in comments and include examples when useful.

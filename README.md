# btimeline

## Generate and validate a draft

`prepare` runs `generate`, YAML schema/semantic validation, and raw-log replay in order. It accepts the same options as `generate`, including group selection and generation mode. The boolean `--convert` flag defaults to false; add it to convert to cactbot text after validation and replay succeed.

```sh
btimeline prepare logs/example --mode raid -o out/example.yaml
btimeline prepare logs/example --mode raid --convert -o out/example.yaml
btimeline prepare logs/example --name "Example Fight" --encounter 105 --difficulty 101 -o out/selected.yaml
```

| Output                | Purpose                        |
| --------------------- | ------------------------------ |
| `example.yaml`        | Editable timeline draft        |
| `example.report.json` | Detailed generation evidence   |
| `example.report.md`   | Readable generation report     |
| `example.replay.json` | Detailed input replay results  |
| `example.replay.md`   | Readable input replay report   |
| `example.txt`         | Cactbot text, with `--convert` |

Use a new output name for each run; existing outputs are not overwritten. With `--convert`, the YAML output must differ from the sibling `.txt` path, and existing text is checked before generation. Keep outputs outside the input log directory. A replay failure returns an error and retains the draft and diagnostic reports for inspection. Successful input replay checks the logs used to generate the draft; it does not establish holdout or cactbot runtime compatibility. The separate `convert` command remains available for edited drafts.

## Verification

Input loading, pairwise alignment, and independent pull replay use Rayon workers. Reports retain input order. Set `RAYON_NUM_THREADS` to limit CPU and memory use, for example:

```sh
RAYON_NUM_THREADS=4 btimeline prepare logs/example -o out/example.yaml
```

| Check                    | Command                              | Toolchain   |
| ------------------------ | ------------------------------------ | ----------- |
| Cast start pairing tests | `cargo test generate::pairing::tests` | Stable Rust |
| Rust tests               | `cargo test`                         | Stable Rust |

Cast pairing tests cover the last eligible start, exact actor/instance/ability matching, timestamp boundaries, and preservation of all unselected starts. Property tests exercise arbitrary pending-start lists; log-loading tests cover cancellation, interleaved instances, duplicate completions, and equal-time source order.

Runtime contracts for P4 grouping and wipe handling use [contracts](https://github.com/x52dev/contracts).

Generated drafts include an `InCombat` entry sync. See [input-to-cactbot verification](docs/cactbot-e2e.md) for the consumer checks, fixes, and ACT capture assumptions.

Converted cactbot timelines begin with the selected reset rows; both use `window 0,1000000 jump 0`.

| Generation mode  | Scope                  | Reset rows                                     |
| ---------------- | ---------------------- | ---------------------------------------------- |
| `raid` (default) | Full fight             | Wipe (`ActorControl`, command `4000000F`)      |
| `dungeon`        | Observed boss segments | Area clear (`SystemLogMessage`, id `7DE`) only |
| `alliance`       | Same as `dungeon`      | Wipe and area clear                            |

Example: `btimeline generate logs/example --mode alliance -o draft.yaml`. Hand-written YAML can select the same resets with `resetOn`:

| `resetOn`           | Reset behavior                                  |
| ------------------- | ----------------------------------------------- |
| Omitted or `[wipe]` | Wipe only (default; raid drafts omit the field) |
| `[areaClear]`       | Area clear only (dungeon drafts)                |
| `[wipe, areaClear]` | Wipe and area clear (alliance drafts)           |
| `[]`                | No automatic reset                              |

| Multiple boss sections (`dungeon` / `alliance`) | Behavior                                                                                                           |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Timeline clock                                  | Separate ranges on a 1000-second grid, with room for observed duration and lookahead                               |
| Entry sync                                      | First safe cast in each section has a window reaching back to zero, so it can select its boss after area clear     |
| Evidence                                        | `bossSections` records offsets and entry indices; `bossSectionChecks` audits all raw training casts after rebasing |
| Overlapping bosses                              | Share one section; existing single-section P7/P8 clocks remain unchanged                                           |

## Multi-pull alignment (P4)

`btimeline align FILE FILE [FILE ...]` prints pairwise boss-anchor segments, ordered signal slots, repeated-anchor candidates, divergent paths, and wipe-censored suffixes as JSON. See [P4 alignment results](docs/p4-alignment.md).

## Branches and phases (P7)

`btimeline generate LOG_DIRECTORY --lookahead 30 -o draft.yaml` compiles observed discriminated paths into label/sync jumps and derives transition windows from corrected clocks. Candidates must pass raw-signal replay; unsafe candidates retain the common draft with rejection evidence. See [P7 policy and verification](docs/p7-branches.md).

## Conditional repeats (P8)

The same `generate` command compresses an independent adjacent repeat only when actor/instance/start/targetability context agrees, distinct exits are reached in both wipe and clear reports, and raw-signal replay passes. Insufficient evidence keeps finite rows and rejection reasons in JSON/Markdown. See [P8 policy and verification](docs/p8-repeats.md).

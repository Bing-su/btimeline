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

Use a new output name for each run; existing outputs are not overwritten. With `--convert`, the YAML output must differ from the sibling `.txt` path, and existing text is checked before generation. Keep outputs outside the input log directory. An empty draft cannot pass replay; a failure returns an error and retains the draft and available diagnostic reports for inspection. Successful input replay checks the logs used to generate the draft; it does not establish holdout or cactbot runtime compatibility. The separate `convert` command remains available for edited drafts.

## Phase schedules

Raid inputs with a consistent phase order and distinct observed boss opening casts use phase schedules before branch mining. Mechanics align by phase, source, name, and elapsed time from the opening cast. This prevents repeated IDs and helper packet order from shifting a mechanic into another round. Inputs without those anchors continue through the existing path compiler.

| Evidence                                                         | Generated behavior                                                                     |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Same mechanic, fixed time, different IDs                         | One mechanic row with an ID array                                                      |
| Same round split by timing drift                                 | Reconcile samples within a 1.5-second span before measuring occurrence frequency       |
| Same mechanic and observed ID attributed to different sources    | One mechanic row with observed source aliases                                          |
| Mutually exclusive effects at the same time                      | One displayed alternative, such as `Future's End / Past's End`                         |
| Frequent completion observed in at least 80% of reached segments | Predictive row; occurrence is mandatory only if invariant                              |
| Conditional or unstable completion                               | Hidden `--reference--` row with raw evidence retained                                  |
| Multiple target/helper packets                                   | All packets retained; one packet owns the internal clock correction                    |
| Nearby mechanics whose packet order varies                       | Keep both forecasts and let the later row own the clock correction                     |
| New instance of a phase opening cast inside an FFLogs phase      | Restore the repeated phase segment; recorded arena reload stops replay before re-entry |

`outputCoverage.predictiveEventIndices` measures prediction coverage separately from `representedEventIndices`. Conditional reference retention alone does not establish a useful forecast. See [the algorithm and Dancing Mad comparison](agent/phase-schedules.md).

After `prepare --convert`, measure predictions in the actual cactbot engine using an existing cactbot checkout with its dependencies installed:

```sh
TS_NODE_TRANSPILE_ONLY=true \
TS_NODE_PROJECT=/home/ks2515/workspace/cactbot/tsconfig.json \
node --loader /home/ks2515/workspace/cactbot/node_modules/ts-node/esm.mjs \
  scripts/verify_phase_schedule.mjs out/example.yaml /home/ks2515/workspace/cactbot
```

The verifier uses cactbot's default six bars and 30-second horizon, probes five seconds before each observed mechanic wave, and checks displayed timing within 2.5 seconds. It also rejects missing or unexpected consumer syncs. Pass a separate replay report as the third argument for holdouts. Network lines and reset lifecycle are reconstructed from FFLogs; live ACT delivery and browser layout remain separate checks.

## Verification

Input loading, pairwise alignment, and independent pull replay use Rayon workers. Reports retain input order. Set `RAYON_NUM_THREADS` to limit CPU and memory use, for example:

```sh
RAYON_NUM_THREADS=4 btimeline prepare logs/example -o out/example.yaml
```

These stages display progress on stderr in terminals; progress is hidden when stderr is redirected. JSON on stdout remains suitable for piping. Invalid collected JSON includes the failing field path, for example `events[2].timestamp`, while trailing values are still rejected.

| Check                    | Command                               | Toolchain   |
| ------------------------ | ------------------------------------- | ----------- |
| Cast start pairing tests | `cargo test generate::pairing::tests` | Stable Rust |
| Rust tests               | `cargo test`                          | Stable Rust |
| CLI process tests        | `cargo test --test cli`               | Stable Rust |

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

| Dungeon log differences         | Generation and replay behavior                                                                                                                 |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| Missing `sourceInstance`        | Counts as one actor; explicit instance IDs remain in source evidence                                                                           |
| Report-local virtual helper IDs | Match by actor name and role; distinct instances arriving within 100 ms form one observed wave                                                 |
| Boss retries                    | Align the last non-overlapping instance span; replay earlier attempts with their own observed branch, including effects before the arena opens |
| Travel and waiting              | Compile each boss on its own corrected clock and reset alignment anchors at boss boundaries                                                    |
| Early boss kills                | Retain observed suffixes from longer clears; shorter clears provide prefix samples and censor the remaining completions                        |
| Nested choices                  | Recover observed dependent paths within a boss, e.g. Chort's consecutive Mortifying Flesh directions                                           |
| Repeated helper waves           | Retain raw packet/instance evidence and separate overlapping sync windows, e.g. two-second Surface Missile waves                               |
| Conditional instant helpers     | Helpers seen conditionally without any cast start remain visible with sync disabled, e.g. Launch; they do not select a branch                  |
| Arena opens (`sealType: 3`)     | Model the consumer stopping on combat exit before the next attempt                                                                             |

Unresolved paths remain reported. For example, `btimeline prepare logs/clyteum --mode dungeon --convert -o out/clyteum.yaml` recovers observed boss mechanics and branches. Check `outputCoverage` for omitted raw casts as well as replay results; passing replay alone does not establish complete coverage or cactbot runtime compatibility.

## Multi-pull alignment (P4)

`btimeline align FILE FILE [FILE ...]` prints pairwise boss-anchor segments, ordered signal slots, repeated-anchor candidates, divergent paths, and wipe-censored suffixes as JSON. See [P4 alignment results](docs/p4-alignment.md).

## Branches and phases (P7)

`btimeline generate LOG_DIRECTORY --lookahead 30 -o draft.yaml` compiles observed discriminated paths into label/sync jumps and derives transition windows from corrected clocks. Candidates must pass raw-signal replay; unsafe candidates retain the common draft with rejection evidence. See [P7 policy and verification](docs/p7-branches.md).

## Conditional repeats (P8)

The same `generate` command compresses an independent adjacent repeat only when actor/instance/start/targetability context agrees, distinct exits are reached in both wipe and clear reports, and raw-signal replay passes. Insufficient evidence keeps finite rows and rejection reasons in JSON/Markdown. See [P8 policy and verification](docs/p8-repeats.md).

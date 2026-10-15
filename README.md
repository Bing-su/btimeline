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

## Timeline quality

Generation prefers a normal, repeatable fight over a transcription of every supplied log. Inputs are validated before selection; source files are never changed.

| Available evidence | Generation inputs |
| --- | --- |
| At least three clears | Use clears; exclude wipe logs from inference |
| At least three uninterrupted clears | Also exclude successful logs containing earlier boss attempts |
| Fewer than three clears, but three substantial progression pulls | Exclude wipes with fewer than one quarter of the longest boss-start trace |
| Insufficient evidence for selection | Retain the available pulls |

Excluded files still receive diagnostic replay. `inputSelection` records paths, reasons, and hashes. `replay.validation.replay` checks inputs that were not excluded, while `allInputsPassed` includes diagnostics. New holdout files always gate replay; exclusion applies only to the exact recorded file and hash. Clear-based generation does not promise wipe-only enrage paths.

| Event evidence | Output |
| --- | --- |
| Supported, predictable mechanic or observed branch | Timeline row |
| Conditional completion below schedule support, or an unstarted conditional helper | `excludedSignals` evidence; no timeline row |
| Regular cast cues delimit overlapping waves of identical mandatory hits | Separate castbar and hidden timing cues from `xN` damage rows; duration measures first to last hit |
| At least three consecutive identical, mandatory phase-schedule hits within 2.5 seconds, all already unsynced | One `xN` row with a duration; all raw packets remain represented |
| Different mechanic between hits, different IDs, optional hits, or independent sync | Keep separate rows |

`outputCoverage` distinguishes represented, intentionally excluded, and accidentally omitted event indices. An exclusion is not prediction coverage. See [the cactbot comparison and limits](agent/timeline-quality.md).

## Raid checkpoints

Raid directories with separate checkpoint encounters, such as R12S P1 and P2, generate one timeline by default. Each encounter keeps its own inference inputs and clock; its opening cast selects the correct range, including direct entry after a checkpoint wipe.

```sh
btimeline prepare logs/r12s --convert -o out/r12s.yaml
btimeline prepare logs/r12s --encounter 105 --convert -o out/r12s-p2.yaml
```

| Required evidence | Purpose |
| --- | --- |
| Shared game zone, difficulty and boss identity | Reject unrelated fights |
| Consistent encounter order within shared reports | Establish segment order without guessing from encounter numbers |
| Stable opening cast unique to each encounter | Select the segment during a continuous run or direct checkpoint entry |

`segments` records each local draft, its report, clock offset and entry mapping. Clocks occupy separate 1000-second ranges; this does not estimate the real intermission duration. Unsupported combinations retain the explicit `--encounter` selection error.

## Phase schedules

Raid inputs with a consistent phase order and distinct observed boss opening casts use phase schedules before branch mining. Mechanics align by phase, source, name, and elapsed time from the opening cast. This prevents repeated IDs and helper packet order from shifting a mechanic into another round. Inputs without those anchors continue through the existing path compiler.

| Evidence                                                         | Generated behavior                                                                     |
| ---------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| Same mechanic, fixed time, different IDs                         | One mechanic row with an ID array                                                      |
| Same round split by timing drift                                 | Reconcile samples within a 1.5-second span before measuring occurrence frequency       |
| Same mechanic and observed ID attributed to different sources    | One mechanic row with observed source aliases                                          |
| Mutually exclusive effects at the same time                      | One displayed alternative, such as `Future's End / Past's End`                         |
| Frequent completion observed in at least 80% of reached segments | Predictive row; occurrence is mandatory only if invariant                              |
| Conditional or unstable completion                               | Exclude from the timeline; retain raw evidence in `excludedSignals`                     |
| Multiple target/helper packets                                   | All packets retained; one packet owns the internal clock correction                    |
| Nearby mechanics whose packet order varies                       | Keep both forecasts and let the later row own the clock correction                     |
| New instance of a phase opening cast inside an FFLogs phase      | Restore the repeated phase segment; recorded arena reload stops replay before re-entry |

`outputCoverage.predictiveEventIndices` measures prediction coverage separately from `representedEventIndices`. Schedule support requires observations in at least two input files; repeated attempts in one file cannot supply that support alone.

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
| Conditional instant helpers     | Exclude helpers without stable cast starts, e.g. Launch, and retain their packet evidence in `excludedSignals`                                  |
| Arena opens (`sealType: 3`)     | Model the consumer stopping on combat exit before the next attempt                                                                             |

Unresolved paths remain reported. For example, `btimeline prepare logs/clyteum --mode dungeon --convert -o out/clyteum.yaml` recovers observed boss mechanics and branches. Check `outputCoverage` for omitted raw casts as well as replay results; passing replay alone does not establish complete coverage or cactbot runtime compatibility.

## Multi-pull alignment (P4)

`btimeline align FILE FILE [FILE ...]` prints pairwise boss-anchor segments, ordered signal slots, repeated-anchor candidates, divergent paths, and wipe-censored suffixes as JSON. See [P4 alignment results](docs/p4-alignment.md).

## Branches and phases (P7)

`btimeline generate LOG_DIRECTORY --lookahead 30 -o draft.yaml` compiles observed discriminated paths into label/sync jumps and derives transition windows from corrected clocks. Candidates must pass raw-signal replay; unsafe candidates retain the common draft with rejection evidence. See [P7 policy and verification](docs/p7-branches.md).

## Conditional repeats (P8)

The same `generate` command compresses an independent adjacent repeat only when actor/instance/start/targetability context agrees, distinct exits are reached in both wipe and clear reports, and raw-signal replay passes. Insufficient evidence keeps finite rows and rejection reasons in JSON/Markdown. See [P8 policy and verification](docs/p8-repeats.md).

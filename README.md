# btimeline

## FFLogs collection and downloads

The CLI reads `.env` in the current directory before parsing arguments. Existing environment variables take precedence; explicit `--client-id`/`--client-secret` flags still work. Keep credentials in the gitignored `.env` or the process environment.

| Command | Purpose |
| --- | --- |
| `fflogs-collect` | Discover report/fight candidates and save a JSON plan without downloading events |
| `fflogs-download` | Download a saved plan; validate and skip existing complete fight files |
| `fflogs` | Existing direct download, `--all`, and `--list` interface |

```sh
# Discover reports through encounter rankings; gather completed pulls, including wipes.
btimeline fflogs-collect --encounter 105 --difficulty 101 --max-reports 5 --max-fights 50 -o out/r12s-plan.json

# Or collect candidates from explicitly chosen reports, optionally filtering by encounter/difficulty.
btimeline fflogs-collect REPORT_CODE OTHER_REPORT_CODE --encounter 105 -o out/chosen-plan.json

# Review the plan, then download. Repeat the same command to resume.
btimeline fflogs-download out/r12s-plan.json -o logs/new-r12s --limit 10

# Existing commands retain their behavior.
btimeline fflogs REPORT_CODE --list
btimeline fflogs REPORT_CODE FIGHT_ID -o logs/example
btimeline fflogs REPORT_CODE FIGHT_ID --all -o logs/example
```

| Limit or output | Behavior |
| --- | --- |
| `--page`, `--pages`, `--partition`, `--region` | Select ranking pages/partition/region; defaults to one page starting at 1. Rankings mainly expose clears; use explicit reports for targeted progression evidence |
| `--max-reports 10`, `--max-fights 100` | Bound discovery. The plan records truncation and individual failures; existing plans are never replaced |
| `--max-requests 100` | Both new commands bound GraphQL HTTP attempts, including the quota probe and retries; OAuth token requests are separate |
| `--max-points 1000` | Both new commands stop before another request after observing this usage. One request can cross the threshold; shared-client activity also counts |
| `--limit 100`, `--max-bytes 2147483648` | Download bounds per invocation. The byte limit bounds newly published files, not bytes received while fetching an individual fight |
| `REPORT_FIGHT.json` | Complete raw log with collection provenance. Invalid or mismatched existing files stop download and are preserved |
| `r12s-plan.download.json` | Sibling status report with downloaded/skipped/remaining counts and failures. Updated on resume; plan and status must be outside the raw-log output directory |

Download stops on a failed fight and retains completed files. Its next invocation retries the first unfinished fight. A failed discovery retains its partial plan and returns failure; collect a new plan with an adjusted page/report selection to extend the sample. Private-user OAuth is not implemented.

## Reproducible quality tooling

Freeze raw log bytes and report-level splits before comparing algorithms. All fights from a report share a split, including checkpoint encounters. Duplicate-upload candidates link their reports into one split; they cannot leak into both training and holdout.

```sh
uv run scripts/audit_timeline_sequences.py > out/input-audit.json
nu scripts/freeze_evaluation_inputs.nu out/input-audit.json > out/evaluation-inputs.csv
```

Reference mappings are JSON objects keyed by encounter/difficulty, for example:

```json
{
  "103-101": "/home/ks2515/workspace/cactbot/ui/raidboss/data/07-dt/raid/r11s.txt",
  "combined-101": "/home/ks2515/workspace/cactbot/ui/raidboss/data/07-dt/raid/r12s.txt"
}
```

Use the actual encounter/difficulty keys from your manifest. `--combine-encounters` groups encounters of each difficulty into one checkpoint generation; unsupported combinations fail. The evaluator also accepts `--mode alliance`.

```sh
node scripts/evaluate_quality.mjs out/evaluation-inputs.csv out/candidate-quality \
  --binary target/release/btimeline \
  --cactbot /home/ks2515/workspace/cactbot \
  --references out/references.json

# Optionally run another binary on exactly the same frozen manifest.
node scripts/evaluate_quality.mjs out/evaluation-inputs.csv out/comparison-quality \
  --baseline /tmp/btimeline-baseline --binary target/release/btimeline \
  --cactbot /home/ks2515/workspace/cactbot --references out/references.json
```

The runner writes `quality.json`, `quality.md`, command logs and per-group artifacts. Correctness gates require independent holdouts, YAML validation, raw cast accounting, cactbot's official timeline tests and actual consumer sync checks. It records reference differences and forecast rates separately; these are diagnostic quality measures, not an assertion of human-authored parity. Baseline deltas do not automatically classify editorial changes as improvements. Train-only runs are incomplete in this runner.

| Tool | Evidence and limits |
| --- | --- |
| `scripts/compare_cactbot.mjs` | Uses the official parser and order-aware mechanic matching. Reports active/commented ID evidence, ID alternatives, durations, hidden rows, windows, resolved jumps and reference ignored IDs. Clock differences across branch ranges are diagnostic |
| `scripts/verify_official_cactbot.mjs` | Runs the unmodified upstream timeline tests in a temporary directory using the reference trigger's real translations and timeline triggers. Requires the cactbot checkout and its Node dependencies; does not modify it |
| `scripts/evaluate_quality.mjs` | Joins frozen replay, reference comparison, official checks, selected-input raw coverage and train/holdout consumer forecasts. Records manifest/reference/binary hashes and the cactbot commit |

Standalone comparison uses the same TypeScript loader as consumer replay:

```sh
TS_NODE_TRANSPILE_ONLY=true \
TS_NODE_PROJECT=/home/ks2515/workspace/cactbot/tsconfig.json \
node --loader /home/ks2515/workspace/cactbot/node_modules/ts-node/esm.mjs \
  scripts/compare_cactbot.mjs out/fight.txt /path/to/reference.txt \
  /home/ks2515/workspace/cactbot -o out/fight.comparison.json

node scripts/verify_official_cactbot.mjs out/fight.txt \
  /home/ks2515/workspace/cactbot /path/to/reference.txt out/fight.official.json
```

Raw coverage distinguishes representation, intentional exclusion and unexplained omissions. Dungeon/alliance raw accounting uses the reported boss spans and retains outside-scope indices explicitly. Forecast probes still derive from generated evidence, so inspect independent raw coverage and reference-only mechanics as well as the forecast rate. Locale/runtime/browser and live ACT delivery require separate validation.

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
| Frozen report splits | `nu scripts/test_freeze_evaluation_inputs.nu` | Nushell |
| Replay evaluator and failure propagation | `nu scripts/test_evaluate_replay.nu target/debug/btimeline` | Nushell |
| Reference matching | `node scripts/test_compare_cactbot.mjs` | Node |
| Raw coverage and quality metrics | `node scripts/test_evaluate_quality.mjs` | Node |
| Quality runner with real consumer and expected official-test failure | `node scripts/test_evaluate_quality.mjs /home/ks2515/workspace/cactbot` | Node + cactbot dependencies |

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

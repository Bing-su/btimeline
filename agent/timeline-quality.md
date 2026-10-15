# Timeline selection and cactbot references

The generator targets useful forecasts of normal progression. It no longer requires a timeline row for every observed reaction or a vote from every supplied pull. Selection uses existing FFLogs metadata and cast evidence; no encounter IDs or ability lists are hardcoded.

## Reference decisions

The local original cactbot checkout was inspected at `/home/ks2515/workspace/cactbot/ui/raidboss/data/07-dt`.

| Reference                                                                           | Observed editorial choice                                                                     | Generator behavior                                                                                                         |
| ----------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `ultimate/dancing_mad.txt` ignored-ability comments                                 | Failed towers, player-triggered explosions, VFX and damage copies do not all become forecasts | Drop statistically conditional effects and unstarted conditional helpers; retain reasons and source indices                |
| `raid/r11s.txt` ignored-ability comments                                            | Missed soaks and damage from standing in a tornado differ from the normal mechanic            | Prefer a clear cohort over progression wipes when three clears exist                                                       |
| `raid/r12s.txt`                                                                     | Checkpoint sections share a file and select their own clocks                                  | Combine observed ordered encounters with unique opening casts; preserve direct P2 entry                                    |
| Dancing Mad `Chaotic Flood x4`, `Stray Apocalypse x6`, and R11S `Massive Meteor x5` | A rapid sequence can occupy one bar with a duration                                           | Use repeated cast cues for overlapping waves, then compact uninterrupted unsynced hits; retain every packet and hit offset |

These references guide general rules. Their ignored IDs are not copied into an encounter-specific allowlist or denylist. Stable VFX and consistently triggered mistakes can still look like real mechanics in FFLogs and need human editing.

## Evidence contract

| Field                                    | Meaning                                                                     |
| ---------------------------------------- | --------------------------------------------------------------------------- |
| `inputSelection.selected`                | Files used for inference, after deterministic selection                     |
| `inputSelection.excluded`                | File path, hash, reason, kill status and boss cast-start count              |
| `excludedSignals[].slot.samples`         | Original source indices for effects intentionally omitted from the timeline |
| `outputCoverage.representedEventIndices` | Actual timeline evidence, including all packets of grouped bursts           |
| `outputCoverage.predictiveEventIndices`  | Packets supporting visible phase-schedule forecasts                         |
| `outputCoverage.excludedEventIndices`    | Intentionally excluded packets, separate from representation                |
| `outputCoverage.omittedEventIndices`     | Remaining unexplained loss; must be investigated                            |
| `slots[].burstOffsetsMs`                 | Expected hit offsets from the first hit of a grouped burst                  |
| `pulls[].excludedFromGeneration`         | Replay is diagnostic for this exact excluded input                          |
| `replay.validation.replay`               | Non-excluded replay inputs passed                                           |
| `replay.validation.allInputsPassed`      | All replay inputs passed, including diagnostic files                        |

The exact excluded path and hash must match before a replay failure becomes diagnostic. An unrelated holdout is never excused by this policy. Invalid source JSON and changed evidence remain errors.

## Limits

| Limit                                                                 | Consequence                                                                                |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Clear cohort does not contain a wipe-only enrage or rare failed route | That route can fail diagnostic replay; it is not claimed as supported                      |
| Fewer than three clears                                               | Retain progression evidence except very short prefixes with sufficient longer observations |
| No explicit boss retry evidence in phase metadata                     | A merged raid retry may remain; phase schedules still recover its observed reset           |
| Burst grouping is conservative and currently phase-schedule-only      | Branch-compiled timelines keep their individual hit rows                                   |
| Reconstructed FFLogs network packets                                  | Consumer tests do not establish live ACT delivery or browser layout                        |

## Initial selection verification, 2026-10-15

This snapshot predates the checkpoint composition and cast-cue corrections below.

The supplied corpus contains 280 files in five encounter groups. Selection retained 77 files. An independent raw-event comparison found 28,008 eligible casts in those selected files: 27,414 represented, 594 intentionally excluded, and zero unexplained omissions. Represented and excluded index sets are disjoint. Excluded files are outside that cast denominator.

| Encounter   | Supplied → selected files |     Previous → current rows | Intentionally excluded casts | Selected-input replay |
| ----------- | ------------------------: | --------------------------: | ---------------------------: | --------------------- |
| Dancing Mad |                   56 → 16 |                   427 → 338 |                          116 | 16/16                 |
| Clyteum     |                   21 → 16 |                   144 → 115 |                           14 | 16/16                 |
| R11S        |                    44 → 5 |                   239 → 184 |                          464 | 5/5                   |
| R12S P1     |                   45 → 14 | Baseline not measured → 187 |                            0 | 14/14                 |
| R12S P2     |                  114 → 26 | Baseline not measured → 159 |                            0 | 26/26                 |

Dancing Mad has twelve grouped bursts. R11S's final Heartbreak Kick rows are retained even without FFLogs phase metadata; they were initially lost after changing the input cohort. Replay evidence now records all packets of a holdout mechanic separately from the packet that owns its clock correction.

The actual cactbot parser/controller/engine and timer callbacks were exercised with reconstructed network lines. All 77 selected inputs have zero missing or unexpected consumer syncs. Diagnostics remain visible: the excluded R11S inputs produce 376 unexpected consumer syncs, and excluded R12S P2 inputs produce two. These inputs do not pass the selected-input gate by being relabeled as successful.

| Selected encounter group | Displayed five seconds before the mechanic |  Rate |
| ------------------------ | -----------------------------------------: | ----: |
| Dancing Mad              |                              5,463 / 5,700 | 95.8% |
| Clyteum                  |                              1,364 / 1,428 | 95.5% |
| R11S                     |                                  772 / 813 | 95.0% |
| R12S P1                  |                              1,346 / 2,049 | 65.7% |
| R12S P2                  |                              3,397 / 3,628 | 93.6% |

These probes use cactbot's default six bars and 30-second horizon. Targets come from generated mechanic evidence; the rates are not a comparison with every manually authored cactbot row. R12S P1's forecast visibility remains weak. Human-authored quality parity has not been established.

For holdouts, all selected files belonging to one report were removed before regenerating each encounter. The independent clear from that report passed raw replay and consumer sync checks for Dancing Mad, Clyteum, R12S P1 and R12S P2. R11S `qBMgh3wGy8jcNAxH / fight 4` failed branch/sample correspondence: eight wrong matches, seven wrong jumps, and eleven window misses. Its failure remains a validation failure and requires further branch-generalization work.

| Automated check                                      | Result                                            |
| ---------------------------------------------------- | ------------------------------------------------- |
| `cargo test --locked`                                | 361 unit tests and three CLI process tests passed |
| `cargo clippy --all-targets --locked -- -D warnings` | Passed                                            |
| `cargo +nightly fmt --check`                         | Passed                                            |
| Working-tree and staged diff whitespace checks       | Passed                                            |
| Consumer verifier JavaScript syntax                  | Passed                                            |

This initial run wrote artifacts under `out/quality/` (gitignored); those files are no longer present in the current workspace.

## Checkpoint and burst corrections, 2026-10-15

R12S P1/P2 now generate one file without `--encounter`. The encounters retain independent local drafts and evidence, with P2 on the 1000-second clock. Hidden opening selectors support both continuous transitions and direct checkpoint entry. Composition requires a common zone, difficulty and boss identity, consistent report-local order, and unique opening casts; unrelated groups still require explicit selection.

Original cactbot also selects P2 immediately with ActorControl/AddedCombatant signals. FFLogs does not establish those raw signals here, so generated selection occurs on P2's first observed boss cast (about 11 seconds into the pull). Earlier preview bars can still belong to P1 during direct P2 entry.

Stray Apocalypse previously mixed BB3B/BB3C cast cues with BB3D damage, and adjacent-row compaction split waves into partial `x3` bars. Cast-start evidence now separates these families. Regular cue periods delimit six overlapping `x6` waves; hidden recurring cues use non-overlapping sync windows. Durations are measured from the first hit to the last (2.5 seconds for the first five waves, 2.6 for the sixth), rather than copied from cactbot's 2.4-second annotation. Shared endpoint packets remain represented in both wave samples.

| Current artifact          |    Selected inputs | Rows | Raw eligible casts | Represented / intentionally excluded / omitted | Actual cactbot missing / unexpected syncs on selected inputs |
| ------------------------- | -----------------: | ---: | -----------------: | ---------------------------------------------- | ------------------------------------------------------------ |
| `out/followup/dancingmad` |                 16 |  333 |             12,054 | 11,729 / 325 / 0                               | 0 / 0                                                        |
| `out/followup/r12s`       | 40 (14 P1 + 26 P2) |  348 |             11,179 | 11,179 / 0 / 0                                 | 0 / 0                                                        |

`out/followup/coverage-audit.json` independently compares raw enemy non-melee cast indices with represented/excluded sets and checks that the sets are disjoint. YAML, text, generation evidence, raw replay and actual cactbot results share the artifact basenames above. R12S excluded-input diagnostics still contain two unexpected consumer syncs; they are not reported as passing.

Regression tests cover direct entry into either checkpoint, a continuous P1-to-P2 clock transition, rejection of four unproven combinations, and six overlapping waves with an independent replay input. Full verification passes 367 unit tests, three CLI tests, Clippy with warnings denied, formatting, diff whitespace and verifier syntax checks. Live ACT capture and browser layout remain unverified.

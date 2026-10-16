# Autonomous generation evaluation — 2026-10-15

Baseline: `cff34eb`. Cactbot reference and consumer: `e79ea7f16093be5b80cda0e2d5bc8eb899d6fb4b`.
Inputs are frozen by SHA-256 and split by report before generation. Existing logs and FFLogs credentials remain outside Git.

## Repairs

| Cause                                                                  | Repair                                                                                                                    | Acceptance evidence                                                                             |
| ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| A boss castbar and its subsequent attack sequence share a display name | Separate multi-hit sequences before the next boss cast; retain single damage-copy aliases                                 | Synthetic castbar followed by three attacks, single-copy aliases and existing helper-copy tests |
| FFLogs omits phases for a fixed sequence with permuted attacks         | Require stable boss cast-cue prefixes from three independent reports and a constant per-ID multiset in a boss permutation | Three training orders and an unseen order of known IDs                                          |
| Late drift moves a repeated packet closer to the next round            | Assign generation and holdout evidence against the preceding observed boss cast cue                                       | Drift exceeding the pulse interval, plus compacted burst onset                                  |
| Boss and helper attacks share a source name and mechanic name          | Include actor role in round matching; cross-role aliases need a shared ID                                                 | Same-name boss/helper casts with independent drift                                              |
| Helper reactions or sync collisions appear to support a schedule       | Require boss permutation evidence and replay every training input before accepting an inferred schedule                   | Helper-only and friendly-opening-collision rejection tests                                      |

Finite ID arrays describe permutations; no encounter IDs or ability lists were added to generation policy. Unknown holdout IDs remain coverage failures.

## R11S focused comparison

Four independent clears train the draft; `qBMgh3wGy8jcNAxH / 4` is held out in both versions. The following figures use identical input bytes, selection policy and cactbot consumer probes.

| Measure                                                       |      Baseline |      Repaired |
| ------------------------------------------------------------- | ------------: | ------------: |
| Represented training casts                                    | 1,392 / 1,758 | 1,737 / 1,758 |
| Intentionally excluded training casts                         |           366 |            21 |
| Holdout casts without representation                          |           119 |             0 |
| Holdout wrong matches / jumps / window misses                 |    8 / 7 / 11 |     0 / 0 / 0 |
| Holdout missing / unverified rows                             |         4 / 7 |         0 / 0 |
| Missing or unexpected cactbot consumer syncs, train + holdout |            11 |             0 |
| Five-second consumer forecast rate                            |        94.36% |        97.74% |
| Generated rows / reference-only rows                          |     187 / 143 |     174 / 150 |
| Official cactbot test failures                                |             1 |             1 |

Reference-only rows increased. The repair improves measured correspondence and visibility, without establishing parity with the authored timeline. Official tests still reject missing translations, including FFLogs `unknown_b404`; the quality command correctly exits unsuccessfully.

Two additional R11S reports were downloaded after the initial repair and kept out of training. `T2DwyX6MJQYqWK8h / 8` has no replay errors or unrepresented casts. `bw3qKBhC8tdR4ykf / 7` exposes two previously unseen Arcadion Avalanche IDs (`B44E`, `B44F`): two missing rows and two unrepresented casts remain. Both logs have zero missing/unexpected consumer syncs **for represented evidence**; this does not erase the raw coverage failure.

## Expanded encounter corpus

Collection plans and event downloads remain separate commands. R9S and R10S each have five clears from five reports: four training reports and one holdout report. The ten downloads contain 230,472 events in total.

| Encounter                               | Held-out report / fight | Baseline → repaired unrepresented casts, train + holdout | Remaining failure                                                                 |
| --------------------------------------- | ----------------------- | -------------------------------------------------------: | --------------------------------------------------------------------------------- |
| R9S, Vamp Fatale (`101 / 101`)          | `vZjf27x1ha9CFM3n / 36` |                                                  304 → 2 | Sadistic Screech missing; two raw casts unrepresented; missing translations       |
| R10S, Red Hot / Deep Blue (`102 / 101`) | `tfgDFL4vWjhm7GwV / 4`  |                                                  91 → 91 | One unverified row, ten consumer sync errors, raw omissions, missing translations |

R9S also changes represented training casts from 1,951 to 1,567 and exclusions from 28 to 412. Its forecast rate rises from 49.87% to 100% **on generated targets**, so that rate alone is not evidence of better mechanic completeness. R10S forecast visibility remains 97.60%; it is an unresolved case.

The original 280-log report split retains the following selected-input results. Excluded inputs are replayed diagnostically and do not count as successful selected inputs.

| Group       | Selected training passes, baseline → repaired | Holdout passes, baseline → repaired |
| ----------- | --------------------------------------------: | ----------------------------------: |
| Dancing Mad |                             14 / 14 → 14 / 14 |                       3 / 3 → 3 / 3 |
| R11S        |                                 4 / 4 → 4 / 4 |                       0 / 2 → 0 / 2 |
| R12S P1     |                             11 / 11 → 11 / 11 |                     0 / 10 → 0 / 10 |
| R12S P2     |                             23 / 23 → 23 / 23 |                   16 / 20 → 16 / 20 |
| Clyteum     |                             11 / 11 → 11 / 11 |                       4 / 5 → 4 / 5 |

This broader R11S split holds out short wipes rather than the clear in the focused comparison. Its unrepresented holdout casts decrease from 267 to 35, but both wipes still fail; their results must not be substituted for the focused clear's result.

## Reproduction artifacts

| Artifact under `out/autonomous/`                                                | Purpose                                                                                                                                                        |
| ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `r11s-inputs.csv`, `new-r11-holdouts.csv`                                       | Frozen R11S input identities and report splits                                                                                                                 |
| `new-encounters.csv`                                                            | Frozen R9S/R10S input identities and report splits                                                                                                             |
| `new-101-plan.json`, `new-102-plan.json`                                        | Metadata collection results before downloading events                                                                                                          |
| `new-101-clear-plan.json`, `new-102-clear-plan.json`                            | One clear per independent report; downloads have sibling status files                                                                                          |
| `r11s-baseline/quality.json`, `r11s-results/quality.json`                       | Focused baseline and final replay, raw coverage, official tests and consumer checks                                                                            |
| `new-encounters-quality/quality.json`, `new-encounters-results/quality.json`    | Baseline measurements from the earlier comparison; final candidate measurements from `new-encounters-results`                                                  |
| `raid-baseline/`, `raid-results/`, `dungeon-baseline/`, `dungeon-results/`      | Original 280-log report splits, including excluded-input diagnostics                                                                                           |
| `r11s-results/new-holdout.replay.json`, `r11s-results/new-holdout.cactbot.json` | Additional R11S holdout coverage and consumer diagnostics                                                                                                      |
| `cleanup.json`                                                                  | Explicit paths removed during intermediate-output cleanup                                                                                                      |
| `regression-summary.json`                                                       | Twenty selected-input comparisons across primary/deduplicated cohorts, train/holdout splits and original encounter groups; zero formerly passing pulls regress |

For example, reproduce the new encounter evaluation with the final release binary:

```sh
node scripts/evaluate_quality.ts out/autonomous/new-encounters.csv out/recheck \
  --binary target/release/btimeline \
  --cactbot /home/ks2515/workspace/cactbot \
  --references out/autonomous/references.json
```

## Storage and verification policy

| Boundary                                | Action                                                                                                                    |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| Each download/evaluation batch          | Measure `du -sb logs out` together so staged hard links are not double-counted                                            |
| New encounter downloads                 | Start with five independent clears and a 150 MB byte budget per batch                                                     |
| Logs above 5 GiB or outputs above 1 GiB | Remove superseded generated attempts first; retain baseline, latest candidate and failing witnesses                       |
| Source deletion                         | Protect every file referenced by an active frozen manifest; do not delete selected evidence to make a failing cohort pass |

Superseded outputs were cleaned twice during this run, reducing output storage by approximately 406 MB while evaluations continued writing new results. The raw corpus remained intact. Cleanup is performed between work batches; no background deletion service was installed.

Rust verification passes: 377 unit tests, four CLI tests, Clippy with warnings denied, nightly formatting and diff checks. Real cactbot parser/controller/timer checks use reconstructed network lines. Live ACT delivery, game sessions and browser rendering remain unverified. Clyteum and R12S already have independent-holdout failures in the baseline; those remain separate improvement targets.

## Remaining defect repairs — 2026-10-16

Baseline: `5c07bdb`. The original binary, frozen R9S/R10S report split, generated artifacts and verification logs are in `out/improvement-20261016/`. Source logs were preserved.

| Confirmed defect                                                                              | Repair                                                                                                               | Regression evidence                                                                                                                   |
| --------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| Report-local virtual actor IDs collide across unrelated enemies and roles                     | Exclude virtual IDs from cross-report role-conflict checks; retain rejection for real game IDs                       | Virtual boss/helper collision accepted; existing real-ID conflict test still passes                                                   |
| A source alias established in one mechanic round is lost in another                           | Share proven aliases only across the same phase, role, cast-pairing state and overlapping ability IDs                | Independent holdout changes the caster of a later round; unrelated phases and IDs stay separate                                       |
| A deferred path has no later safe merge, leaving casts outside the final suffix unrepresented | Preserve remaining casts as sync-disabled `unresolvedPath` evidence; honor correspondence recovered by existing rows | Two final-wave dependent paths retain every cast, with and without phase metadata; empty consensus still fails                        |
| A changed loop sequence aborts replay before diagnostic results are written                   | Mark the expected loop row as required missing and stop extending that attempt                                       | Missing steps, changed sources and targetability write failed replay reports; an excluded wipe cannot abort selected-clear validation |

The last repair also restores combined R12S `prepare`: baseline generation wrote YAML but aborted on excluded `RYVqaCf3zZyc8TpF / 59` before producing the replay report and text. The repaired command passes all 40 selected inputs and records 25 excluded-input replay failures. These failures remain failures.

| Full corpus                | Selected inputs passing raw replay | Eligible casts | Represented | Intentionally excluded | Unexplained omissions | Selected consumer missing / unexpected |
| -------------------------- | ---------------------------------: | -------------: | ----------: | ---------------------: | --------------------: | -------------------------------------: |
| Clyteum                    |                            20 / 20 |          3,646 |       3,632 |                     14 |                     0 |                                  0 / 0 |
| Dancing Mad                |                            16 / 16 |         12,054 |      11,729 |                    325 |                     0 |                                  0 / 0 |
| R11S                       |                              5 / 5 |          2,196 |       2,181 |                     15 |                     0 |                                  0 / 0 |
| R12S, combined checkpoints |                            40 / 40 |         11,179 |      11,035 |                    144 |                     0 |                                  0 / 0 |

`corpus-summary.json` uses `auditCoverage` to count eligible raw enemy casts and verify disjoint represented/excluded sets. `emitted-sample-audit.json` separately checks that all claimed represented indices remain in emitted slot samples across all 81 selected files. The generated row counts are 121, 333, 173 and 280 respectively. Reconstructed consumer packets exercise the real cactbot engine; excluded-input diagnostics and live runtime remain separate from the selected-input gate.

The frozen R9S/R10S comparison uses four training reports and one unseen report per encounter. The `primary` and `withoutDuplicateCandidates` evaluations agree.

| Measure                                      |    Baseline |    Repaired |
| -------------------------------------------- | ----------: | ----------: |
| R9S holdout missing required rows            |           1 |           0 |
| R9S holdout unrepresented casts              |           2 |           1 |
| R10S training unrepresented casts            |          21 |           0 |
| R10S training represented / eligible casts   | 770 / 1,082 | 791 / 1,082 |
| R10S holdout unrepresented casts             |          83 |          83 |
| R10S consumer sync errors, train and holdout |          10 |          10 |

The recovered R10S casts are reference evidence, not additional forecasts. The full quality runner still exits unsuccessfully: R9S has an unseen Massive Impact cast, R10S retains holdout and consumer errors, and official cactbot checks report missing German translations for `Charnel Cell` and `unknown_b578`.

| Further algorithm work                                                | Current evidence                                                                                                                                                                            | Acceptance requirement                                                                                                                                                                              |
| --------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Model joint boss waves and dependent choices before dispatching paths | R10S boss cues differ by at most 909ms across these five logs; two bosses also reverse their 45ms packet order. Relaxing the single-boss cue guard alone did not yield an accepted schedule | Preserve the observed per-boss sequence and multiplicity; eliminate the holdout's two wrong jumps, two dependency violations, one window miss and three unverified rows without excluding its casts |
| Validate completeness against independent mechanic targets            | R9S reports 100% visibility on generated targets while excluding 412 of 1,979 training casts and leaving 95 authored reference rows unmatched                                               | Report raw coverage, exclusions, reference differences and consumer visibility together; generated-target visibility alone cannot establish mechanic completeness                                   |
| Recover full-dungeon section entry and retry correspondence           | Meso Terminal input inspection now accepts all 14 logs, but generation still fails boss-section raw replay on `3AQwNF2Bh8rkxnjZ / 1`                                                        | Reproduce the section failure against raw attempts before changing entry windows; retain the full dungeon as the validation scope                                                                   |

Verification passed: 407 Rust unit tests, four CLI tests, Clippy with warnings denied, nightly formatting, diff checks, Nushell replay/frozen-input regressions, both TypeScript audit/reference tests and strict TypeScript checking. No new dependencies or encounter-specific ability rules were added. Live ACT delivery and browser layout were not tested.

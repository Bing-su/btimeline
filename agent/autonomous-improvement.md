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

---
name: timeline-author
description: Author or improve a btimeline KDL timeline from FFLogs using agent interpretation, source evidence, and offline replay. Use for encounter timeline writing and mechanic interpretation, rather than ordinary generator implementation or code review.
---

# Timeline author

Interpret mechanics and write standalone KDL; use Rust tools to organize observations and check useful advance warning, safe control, and recovery.

## Contracts

Read the scope and provenance sections of the [authoring agreement](../../../docs/AGENT_TIMELINE_AUTHORING.md) when establishing the task. Reuse already-read contracts; revisit relevant sections when semantics or input policy changes:

| Work                                             | Reference                                                          |
| ------------------------------------------------ | ------------------------------------------------------------------ |
| Write KDL or change controls                     | [Timeline schema](../../../docs/TIMELINE_SCHEMA.md)                |
| Judge practical usefulness                       | [Quality criteria](../../../docs/GENERATION.md#실용적인-품질-기준) |
| Interpret replay/forecast signals                | [Replay guide](../../../docs/OFFLINE_REPLAY.md)                    |
| Configure projection, audit or frozen evaluation | [Authored evaluation](../../../docs/AUTHORED_EVALUATION.md)        |

## Workflow

1. Fix development/held-out roles, report/fight identities, input hashes and encounter scope before reading events. Use long development progress as a representative, then seek different abilities, sources and continuations among development inputs. Keep held-out content reserved for the frozen candidate.
2. Validate inputs with `inspect`. Start with bounded `evidence INPUT --source NAME`; investigate a source index with `evidence INPUT --around-index N`. Read the returned count and omitted count before drawing conclusions. Use `mine` for graph questions and `generate` as an optional draft, not mandatory work on every iteration. Preserve input-integrity failures and raw indices.
3. Separate displays from selection, merge and sync controls. For shared signals, concurrent bosses, short choice lead, recurrence or cancellation at a merge, consult [control and coverage decisions](references/control-and-coverage.md). Keep uncertainty local and supported displays useful.
4. Put display/control decisions in KDL and projection/target facts in the evaluation profile. Review unclassified casts as candidates, not automatic missing mechanics. Add useful targets or document an `ignored` reason supported by observations; keep uncertain items open with their scope limits. Record exclusions and their user-visible omissions. Shared tooling belongs in Rust; reuse the common CLI.
5. Run `evaluate MANIFEST --profile PROFILE -o REPORT`, then `review REPORT`. Inspect the first issue and its `evidence` window; keep the full report on disk. Page audits with `next_unclassified_offset` rather than expanding output. Test warning before its own signal, omitted controls, early/late recovery, merge boundary ordering and lifecycle cancellation. Current and hypothetical future diagnostics have different meanings. Record branch/loop coverage separately from total passed traces.
6. Freeze KDL, profile, evaluator and criteria before evaluating held-out events. Copy `profile_sha256` and `evaluator_sha256` from the development report into the held-out manifest. If results guide revision, reclassify those inputs as development and reserve fresh inputs for a new independent claim. Without fresh evidence, state that independent verification is unavailable.

On resuming, read the current candidate, decision record and compact review first. Load raw intervals and contract sections relevant to the change; do not repeatedly dump whole logs, reports or implementation files.

## Completion

Keep an authoring record beside KDL: candidate and evaluation bindings, report/fight plus raw indices, interpretation and control impact, scope/target changes, per-route development/held-out coverage, and remaining uncertainty. Preserve generated draft evidence separately.

Report predictive scores, unclassified review status, exclusions and untested routes separately. Passing selected targets is not complete coverage. Offline projections and synthetic tests do not establish ACT or UI compatibility. Final KDL runs without the profile, evidence or FFLogs metadata.

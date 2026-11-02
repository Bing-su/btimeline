---
name: timeline-author
description: Author or improve a btimeline KDL timeline from FFLogs using agent interpretation, source evidence, and offline replay. Use for encounter timeline writing and mechanic interpretation, rather than ordinary generator implementation or code review.
---

# Timeline author

Use btimeline to extract observations and check execution; interpret mechanics and write the KDL yourself. Aim for useful advance warning, safe control, and recovery at later syncs.

## Establish the task

Resolve the supplied logs, encounter or boss scope, mode, and output location from the request and input inspection. Ask only for missing facts that affect the result. Start with a bounded scope when the task is exploratory, such as one boss.

Read the [authoring agreement](../../../docs/AGENT_TIMELINE_AUTHORING.md) for tool boundaries and output provenance, the [quality criteria](../../../docs/GENERATION.md#실용적인-품질-기준) for acceptance, and the [timeline schema](../../../docs/TIMELINE_SCHEMA.md) before writing KDL. Read the [replay guide](../../../docs/OFFLINE_REPLAY.md) before preparing or interpreting recordings.

## Write and verify

1. Fix training and held-out input roles before interpreting the fight. Record file hashes, report/fight identities, mode, and scope. Work from training observations; reserve held-out encounter content for evaluation after freezing the candidate.
2. Inspect training inputs with `btimeline inspect --json`. Use `mine` for raw occurrence structure and `generate` for an optional draft and generated evidence. Follow current command forms in the authoring agreement. If generation rejects a route, retain that diagnostic and continue from observations; keep input-integrity failures blocking use of that input.
3. Choose a representative training route and identify mechanics that players need to anticipate. Examine divergent intervals in their original context: concurrent casts, alternatives, recurrence, missing observations, and early termination. Use actor/boss scope, neighboring mechanics, and later continuation to test interpretations. An exact-sequence mismatch is a question to investigate, not proof of a different mechanic.
4. Write or edit KDL. Keep the occurrence graph and source observations as evidence while summarizing displays where useful. Distinguish alternatives from abilities that always occur together. Select syncs and branch/loop controls separately from displays; check competing identities and activation windows before widening a sync. A control you cannot justify can remain disabled while supported displays and shared continuation are retained.
5. Keep an authoring record beside the KDL. For each consequential interpretation, link report/fight and raw indices or graph nodes, state the reasoning, and describe uncertainty and display/control impact. Explain intent in KDL comments where the choice would otherwise be surprising. Preserve generated evidence as the unmodified draft baseline; bind the authoring record to the final KDL hash.
6. Validate KDL structure, then replay normalized recordings made from training observations. Fix execution violations and unsafe control. Test advance warning with only earlier control signals supplied, not the future mechanic's own observation. Test selected signal omissions, early/late progression, and recovery at a later representative sync. Report what was and was not checked; a successful command exit alone is insufficient.
7. Freeze the candidate KDL hash and evaluation targets, then evaluate held-out logs. Interpret only after recording the result. If their contents or results guide a revision, reclassify those inputs as development evidence and obtain fresh held-out inputs for a new independent claim. If fresh inputs are unavailable, report independent evaluation as unavailable rather than reusing the same evidence as independent.

Use short feedback cycles on the affected interval. When the remaining issue needs unavailable signals, missing observations, or unsupported execution semantics, keep the supported scope and state the concrete limitation. Report tool gaps separately instead of expanding the task into generator development without authorization.

## Completion

Return the authored KDL path and authoring record, inspected scope, tested forecast/control/recovery cases, remaining uncertainty, and independent-evaluation status. Include reproducible commands and artifact hashes in the record. Claim readiness only for the checked scope; identify unresolved causes and user-visible effects using the quality criteria.

Final KDL executes without the authoring record, FFLogs metadata, or generated evidence. Committing or publishing artifacts follows the user's requested scope.

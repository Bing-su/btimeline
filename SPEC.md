# btimeline v1 specification

## Purpose and scope

| Item              | Contract                                                                                                                        |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Source            | One editable YAML document per cactbot raidboss timeline; authors and tools use the same document.                              |
| Conversion        | Validate the source, then emit a cactbot timeline `.txt` file. Direction: **btimeline → cactbot**.                              |
| Output location   | Determined by the source path and conversion command.                                                                           |
| Runtime semantics | Defined by cactbot's timeline parser.                                                                                           |
| Outside v1        | Parsing cactbot text back into btimeline; generating `timelineTriggers` or `timelineReplace`. Those remain in the trigger file. |

## File format

| Property            | Rule                                                                                         |
| ------------------- | -------------------------------------------------------------------------------------------- |
| Encoding            | UTF-8                                                                                        |
| YAML                | Version 1.2; exactly one document; no duplicate mapping keys                                 |
| Quoting             | Quote ability IDs, names that could be interpreted as YAML scalars, and regular expressions. |
| Unknown keys        | Invalid throughout the v1 data model                                                         |
| Validation sequence | JSON Schema first, then semantic validation                                                  |
| Schema structure    | Discriminated entry variants (`kind`); extra properties rejected                             |

### Top-level fields

| Field           | Type                             | Required | Meaning                                                                                                  |
| --------------- | -------------------------------- | -------- | -------------------------------------------------------------------------------------------------------- |
| `schemaVersion` | integer, exactly `1`             | yes      | Format version                                                                                           |
| `hideNames`     | unique array of nonempty strings | no       | Names emitted as `hideall` commands; defaults to `[]`                                                    |
| `entries`       | ordered array of entries         | yes      | Timeline, annotations, and ability catalogs; order controls comment placement and equal-time event order |

### Entries

| `kind`             | Required fields     | Optional fields                                  | Meaning                                       |
| ------------------ | ------------------- | ------------------------------------------------ | --------------------------------------------- |
| `event`            | `at`, `name`        | `duration`, `sync`, `jump`, `note`               | Timeline event                                |
| `label`            | `at`, `name`        | —                                                | Named jump destination                        |
| `note`             | `text`              | —                                                | Standalone editorial comment at this position |
| `generatorOptions` | at least one option | `targetable`, `ignoredCombatants`, `phaseStarts` | Notes for `make_timeline` regeneration        |
| `syncOrder`        | `enabled`           | —                                                | cactbot sync-order lint directive             |
| `abilityCatalog`   | `abilities`         | `phase`                                          | Encounter ability inventory at this position  |

| Field                     | Constraint                                             | Output                       |
| ------------------------- | ------------------------------------------------------ | ---------------------------- |
| `event.at`, `label.at`    | Finite, nonnegative seconds; at most one decimal place | Timeline time                |
| `duration`                | Finite, positive seconds                               | `duration` keyword           |
| Event or label `name`     | Nonempty; no double quote, newline, or carriage return | cactbot double-quoted string |
| `note.text`, event `note` | Nonempty; multiline allowed                            | One `#` line per text line   |

| Event note case        | Placement                                                                          |
| ---------------------- | ---------------------------------------------------------------------------------- |
| Enabled or absent sync | Inline after the event                                                             |
| Disabled sync          | Immediately before the event, leaving the disabled sync as its only inline comment |

### Sync

| Variant                | Required fields | Optional fields     | Output            |
| ---------------------- | --------------- | ------------------- | ----------------- |
| Network log            | `log`, `fields` | `enabled`, `window` | `LogType { ... }` |
| Raw regular expression | `regex`         | `enabled`, `window` | `sync /.../`      |

Exactly one of `log` and `regex` is allowed.

| Field                        | Constraint or behavior                                                                                                                                                                     |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `enabled`                    | Defaults to `true`. `false` intentionally suppresses synchronization while retaining the event; it is not a proposed sync. Emit the disabled condition after `#` on the event line.        |
| `log`                        | cactbot network-log definition name, such as `Ability` or `StartsUsing`                                                                                                                    |
| `fields`                     | Nonempty mapping; keys must exist for the selected log definition. Each value is a regular-expression string or a nonempty array of such strings. `capture` and `timestamp` are forbidden. |
| `regex`                      | Must be accepted by cactbot's regex parser.                                                                                                                                                |
| `regex`, each `fields` value | A literal `#` is invalid: cactbot treats the remainder of the output line as a comment, even inside a quoted value.                                                                        |
| `window`                     | `[before, after]`; each value is finite, nonnegative seconds with at most one decimal place. When absent, cactbot defaults to 2.5 seconds on each side.                                    |

| Window case              | Output                                                                 |
| ------------------------ | ---------------------------------------------------------------------- |
| Absent                   | Omit `window`.                                                         |
| Present on enabled sync  | `window before,after`; write whole numbers without redundant `.0`.     |
| Present on disabled sync | Emit after `#` with the disabled condition; retained as documentation. |

### Jumps

`jump` has exactly `to` and `when`.

| Field          | Type                           | Output or meaning                                                              |
| -------------- | ------------------------------ | ------------------------------------------------------------------------------ |
| `to`           | Label name or nonnegative time | Destination; numeric `0` pauses the timeline with `when: sync`.                |
| `when: sync`   | —                              | `jump`; execute only when the event's sync matches.                            |
| `when: always` | —                              | `forcejump`; also execute when the timeline reaches the event without a match. |

| Rule                | Requirement                                                                                   |
| ------------------- | --------------------------------------------------------------------------------------------- |
| Sync                | The same event must have an enabled sync.                                                     |
| Zero destination    | `to: 0` with `when: always` is invalid: cactbot does not stop on its unconditional jump path. |
| String destination  | Resolve to exactly one label in the file.                                                     |
| Numeric destination | Follow the same time precision rule as `at`.                                                  |

### Comments and generation metadata

| Source                                                       | Cactbot comment output                      |
| ------------------------------------------------------------ | ------------------------------------------- |
| `generatorOptions.targetable`: array of names                | `# -it ...`                                 |
| `generatorOptions.ignoredCombatants`: array of names         | `# -ic ...`                                 |
| `generatorOptions.phaseStarts`: array of `{ abilityId, at }` | `# -p ID:time ...`                          |
| `syncOrder.enabled: false`                                   | `#cactbot-timeline-lint-disable-sync-order` |
| `syncOrder.enabled: true`                                    | `#cactbot-timeline-lint-enable-sync-order`  |

| Rule                  | Requirement                                                                                                          |
| --------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `note`                | Arbitrary human explanation; tools preserve its position and content. Machine-meaningful comments use typed entries. |
| Generator names       | Quote when needed by `make_timeline`.                                                                                |
| Ignored IDs           | Derived from `abilityCatalog`; there is no `ignoreIds` generator option.                                             |
| Sync-order directives | Every disable must be followed by an enable. Nested or unmatched directives are invalid.                             |

### Ability catalogs

An `abilityCatalog` records abilities observed in an encounter or phase.

| Ability field | Type                         | Required | Meaning                                                         |
| ------------- | ---------------------------- | -------- | --------------------------------------------------------------- |
| `id`          | uppercase hexadecimal string | yes      | Ability ID                                                      |
| `name`        | nonempty string              | yes      | Observed ability name                                           |
| `note`        | nonempty string              | no       | Human explanation                                               |
| `ignored`     | boolean                      | no       | Exclude from log-based timeline generation; defaults to `false` |

| Rule              | Requirement                                                                         |
| ----------------- | ----------------------------------------------------------------------------------- |
| ID uniqueness     | Unique within one catalog; the same ID may appear in different phase catalogs.      |
| `ignored: true`   | Records a generation decision; does not hide an existing event or disable its sync. |
| Multiple catalogs | Preserve their positions in `entries`; no global deduplication.                     |
| Runtime           | Documentation and generator input only; not an event.                               |

For each catalog, emit these comments in order:

| Order | Comment                     | Contents                                                            |
| ----- | --------------------------- | ------------------------------------------------------------------- |
| 1     | `# Phase`                   | Scope marker if `phase` is present                                  |
| 2     | `# -ii ID ...`              | IDs with `ignored: true`, derived from this catalog; omit when none |
| 3     | `# IGNORED ABILITIES`       | Only ignored abilities; omit when none                              |
| 4     | `# ALL ENCOUNTER ABILITIES` | Every ability, including ignored ones                               |

Rows in both tables use `# ID Name` or `# ID Name: Note` and preserve ability source order.

## Conversion rules

| Source                                     | Cactbot text                              |
| ------------------------------------------ | ----------------------------------------- |
| `hideNames: ["--sync--"]`                  | `hideall "--sync--"`                      |
| `{ kind: label, at: 100.0, name: repeat }` | `100.0 label "repeat"`                    |
| Event `at` + `name`                        | `100.0 "Name"`                            |
| Enabled network `sync`                     | `Ability { id: "9CD0" }`                  |
| Disabled network `sync`                    | `#Ability { id: "9CD0" }` after the event |
| `duration: 9.7`                            | `duration 9.7`                            |
| `jump: { to: repeat, when: sync }`         | `jump "repeat"`                           |
| `jump: { to: repeat, when: always }`       | `forcejump "repeat"`                      |

| Stage                            | Output order                                                                                                                                         |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| File                             | All `hideall` commands first; then `entries` in source order.                                                                                        |
| Enabled event                    | Name, sync, duration, window, jump or forcejump.                                                                                                     |
| Disabled sync                    | After active event parts, as an inline `#` comment with its optional window. It cannot carry an active jump because cactbot discards text after `#`. |
| Notes, lint directives, catalogs | Comments only.                                                                                                                                       |

| Output guarantee | Requirement                                                                                         |
| ---------------- | --------------------------------------------------------------------------------------------------- |
| Encoding         | Deterministic UTF-8 text with LF line endings                                                       |
| Fidelity         | Reject input that cannot be encoded faithfully; never silently drop fields or change their meaning. |

## Example

```yaml
schemaVersion: 1
hideNames: ["--sync--"]

entries:
  - kind: generatorOptions
    targetable: ["Black Cat"]

  - kind: note
    text: "The first boss jump is the opening sync."

  - kind: event
    at: 0.0
    name: "--sync--"
    sync:
      log: InCombat
      fields: { inGameCombat: "1" }
      window: [0, 1]

  - kind: label
    at: 100.0
    name: repeat

  - kind: event
    at: 145.0
    name: "Mouser"
    duration: 9.7
    sync:
      log: Ability
      fields: { id: "9441", source: "Black Cat" }
    jump: { to: repeat, when: always }

  - kind: event
    at: 150.0
    name: "Mouser follow-up"
    sync:
      enabled: false
      log: Ability
      fields: { id: "9442", source: "Black Cat" }

  - kind: abilityCatalog
    abilities:
      - id: "9441"
        name: "Mouser"
      - id: "9442"
        name: "Mouser follow-up"
        note: "Extra hit; intentionally not used for syncing"
        ignored: true
```

Relevant emitted lines:

```text
hideall "--sync--"
# -it "Black Cat"
# The first boss jump is the opening sync.
0.0 "--sync--" InCombat { inGameCombat: "1" } window 0,1
100.0 label "repeat"
145.0 "Mouser" Ability { id: "9441", source: "Black Cat" } duration 9.7 forcejump "repeat"
150.0 "Mouser follow-up" #Ability { id: "9442", source: "Black Cat" }
# -ii 9442
# IGNORED ABILITIES
# 9442 Mouser follow-up: Extra hit; intentionally not used for syncing
# ALL ENCOUNTER ABILITIES
# 9441 Mouser
# 9442 Mouser follow-up: Extra hit; intentionally not used for syncing
```

## Validation and compatibility

| Gate               | Checks                                                                                                                                                                                                                                                                         |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| JSON Schema        | Types, required fields, entry variants, unknown keys                                                                                                                                                                                                                           |
| Semantic validator | Time and window precision; timed-entry order while sync-order lint is enabled; unique labels; resolved jumps; valid jump combinations; balanced sync-order directives; ability ID uniqueness per catalog; network-log field names; regular expressions; safe cactbot rendering |
| cactbot parser     | Parse emitted text and reject parser errors.                                                                                                                                                                                                                                   |

| Ordering case | Rule                                  |
| ------------- | ------------------------------------- |
| Untimed entry | Does not reset time-order validation. |
| Equal times   | Allowed; retain source order.         |

| Fixture          | Required coverage                                                                         |
| ---------------- | ----------------------------------------------------------------------------------------- |
| Source-to-output | Active and disabled syncs, labels, jumps, notes, lint directives, and both ability tables |

No cactbot-to-btimeline conversion is required.

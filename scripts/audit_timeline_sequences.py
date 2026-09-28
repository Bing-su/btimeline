#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# ///
"""Audit raw logs, e.g. uv run scripts/audit_timeline_sequences.py > /tmp/btimeline-audit.json.

Boss trace matching reports candidates, not proven branches or runtime compatibility.
"""

import collections
import difflib
import hashlib
import json
from itertools import pairwise
from pathlib import Path


def load_trace(path, data):
    report = data["report"]
    fight = report["fights"][0]
    actors = {actor["id"]: actor for actor in report["masterData"]["actors"]}
    enemies = {actor["id"] for actor in fight["enemyNPCs"]}
    # Boss starts provide sparse structural anchors; helpers remain necessary in a generator.
    events = sorted(
        (
            event
            for event in data["events"]
            if event["type"] == "begincast"
            and event.get("sourceID") in enemies
            and actors[event["sourceID"]]["subType"] == "Boss"
        ),
        key=lambda event: event["timestamp"],
    )
    return {
        "file": str(path),
        "group": (fight["encounterID"], fight["difficulty"]),
        "tokens": [
            (actors[event["sourceID"]]["gameID"], event["abilityGameID"])
            for event in events
        ],
        "seconds": [
            (event["timestamp"] - fight["startTime"]) / 1000 for event in events
        ],
    }


def compare(reference, trace):
    # ponytail: greedy pairwise matching is diagnostic only; use anchor-bounded DP for generation.
    matcher = difflib.SequenceMatcher(
        None, reference["tokens"], trace["tokens"], autojunk=False
    )
    blocks = matcher.get_matching_blocks()
    deltas = [
        trace["seconds"][block.b + offset] - reference["seconds"][block.a + offset]
        for block in blocks
        for offset in range(block.size)
    ]
    return {
        "reference": reference["file"],
        "file": trace["file"],
        "matched": sum(block.size for block in blocks),
        "counts": [len(reference["tokens"]), len(trace["tokens"])],
        "offset_range_seconds": [round(min(deltas), 3), round(max(deltas), 3)]
        if deltas
        else None,
        "differences": [
            {
                "kind": kind,
                "reference": [
                    {
                        "ability": format(token[1], "X"),
                        "at": reference["seconds"][index],
                    }
                    for index, token in enumerate(reference["tokens"][a:b], a)
                ],
                "other": [
                    {"ability": format(token[1], "X"), "at": trace["seconds"][index]}
                    for index, token in enumerate(trace["tokens"][c:d], c)
                ],
            }
            for kind, a, b, c, d in matcher.get_opcodes()
            if kind != "equal"
        ],
    }


def inspect(path):
    raw = path.read_bytes()
    data = json.loads(raw)
    report = data["report"]
    fight = report["fights"][0]
    {a["id"]: a for a in report["masterData"]["actors"]}
    abilities = {a["gameID"] for a in report["masterData"]["abilities"]}
    enemies = {a["id"] for a in fight["enemyNPCs"]}
    events = data["events"]
    casts = [
        e
        for e in events
        if e["type"] == "cast" and e.get("sourceID") in enemies and not e.get("melee")
    ]
    starts = [
        e for e in events if e["type"] == "begincast" and e.get("sourceID") in enemies
    ]
    trace = load_trace(path, data)
    collection = data.get("collection", {})
    # Exact identity checks detect duplicate executions without merging distinct helpers.
    keys = [
        (
            e["timestamp"],
            e.get("sourceID"),
            e.get("sourceInstance"),
            e.get("abilityGameID"),
        )
        for e in casts
    ]
    boss = [
        (format(token[1], "X"), sec)
        for token, sec in zip(trace["tokens"], trace["seconds"])
    ]
    return {
        "file": str(path),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "identity": [report.get("code"), fight["id"]],
        "group": list(trace["group"]),
        "kill": fight.get("kill"),
        "length_seconds": (fight["endTime"] - fight["startTime"]) / 1000,
        "events": len(events),
        "enemy_casts": len(casts),
        "enemy_starts": len(starts),
        "boss_starts": len(boss),
        "language": report["masterData"].get("lang"),
        "complete": collection.get("complete"),
        "page_count": collection.get("pageCount"),
        "manifest_count_matches": collection.get("eventCount") == len(events),
        "final_cursor": collection.get("nextPageTimestamp"),
        "timestamp_inversions": sum(
            a["timestamp"] > b["timestamp"] for a, b in pairwise(events)
        ),
        "out_of_fight": sum(
            not fight["startTime"] <= e["timestamp"] <= fight["endTime"] for e in events
        ),
        "duplicate_cast_keys": len(keys) - len(set(keys)),
        "missing_cast_abilities": sorted(
            {e["abilityGameID"] for e in casts} - abilities
        ),
        "event_types": dict(collections.Counter(e["type"] for e in events)),
        "phase_transitions": fight.get("phaseTransitions"),
        "r12s_choices": [a for a, _ in boss if a in {"B52E", "B52F", "B52B", "B52C"}],
        "r12s_choice_times": [
            [a, t] for a, t in boss if a in {"B52E", "B52F", "B52B", "B52C"}
        ],
        "r12s_tail": [[a, t] for a, t in boss if a in {"B533", "B537"}],
        "boss_prefix": [[a, t] for a, t in boss[:12]],
        "fight_percentage": fight.get("fightPercentage"),
        "boss_percentage": fight.get("bossPercentage"),
        "phase_relative_seconds": [
            (p["startTime"] - fight["startTime"]) / 1000
            for p in fight.get("phaseTransitions") or []
        ],
        "clyteum_order": [a for a, _ in boss if a in {"C3FF", "C400"}],
        "choice_next_boss_offsets": [
            [ability, boss[index + 1][1] - at]
            for index, (ability, at) in enumerate(boss[:-1])
            if ability in {"B52E", "B52F", "B52B", "B52C"}
        ],
        "absolute_start_ms": report["startTime"] + fight["startTime"],
        "absolute_end_ms": report["startTime"] + fight["endTime"],
    }, trace


def main():
    # A runnable sanity check catches accidental loss of alternative and repeated tokens.
    ref = {
        "file": "reference",
        "tokens": [(1, 10), (1, 20), (1, 30)],
        "seconds": [0, 1, 2],
    }
    alt = {
        "file": "alternative",
        "tokens": [(1, 10), (1, 21), (1, 30), (1, 30)],
        "seconds": [0, 1, 2, 3],
    }
    result = compare(ref, alt)
    assert result["matched"] == 2
    assert [change["kind"] for change in result["differences"]] == ["replace", "insert"]
    rows, groups = [], collections.defaultdict(list)
    candidates = collections.defaultdict(list)
    for path in sorted(Path("logs").glob("*/*.json")):
        row, trace = inspect(path)
        rows.append(row)
        groups[trace["group"]].append(trace)
        # Cross-report uploads can describe one pull; retain files and report candidate evidence.
        key = (
            trace["group"],
            row["absolute_start_ms"],
            row["absolute_end_ms"],
            row["kill"],
            tuple(trace["tokens"]),
        )
        candidates[key].append((row, trace))
    assert len({tuple(row["identity"]) for row in rows}) == len(rows), "duplicate pulls"
    assert all(not row["missing_cast_abilities"] for row in rows), (
        "unresolved abilities"
    )
    output = {
        "files": rows,
        "groups": [],
        "duplicate_pull_candidates": [
            {
                "files": [row["file"] for row, _ in matches],
                "max_boss_timing_difference_ms": round(
                    max(
                        abs(at - reference) * 1000
                        for _, trace in matches
                        for at, reference in zip(
                            trace["seconds"], matches[0][1]["seconds"]
                        )
                    ),
                    3,
                ),
            }
            for matches in candidates.values()
            if len(matches) > 1
        ],
    }
    for group, traces in sorted(groups.items()):
        # Compare every pair; a first-file reference can hide reference-selection bias.
        pairs = [compare(a, b) for i, a in enumerate(traces) for b in traces[i + 1 :]]
        output["groups"].append({"group": list(group), "pairs": pairs})
    print(json.dumps(output, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

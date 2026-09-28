"""Probe branch generalization with PM4Py 2.7.23.8; this is not a generator."""

import importlib.metadata
import itertools
import json

import pm4py
from pm4py.objects.log.obj import Event, EventLog, Trace


def log_of(sequences):
    return EventLog(
        [Trace([Event({"concept:name": x}) for x in seq]) for seq in sequences]
    )


cases = {
    "sparse_independent_or_constrained": [
        ["start", "A", "C", "end"],
        ["start", "B", "C", "end"],
        ["start", "B", "D", "end"],
    ],
    "correlated_only": [["start", "A", "C", "end"], ["start", "B", "D", "end"]],
    "all_combinations": [
        ["start", x, y, "end"] for x, y in itertools.product("AB", "CD")
    ],
}
results = {}
for name, seqs in cases.items():
    tree = pm4py.discover_process_tree_inductive(log_of(seqs), noise_threshold=0)
    net, im, fm = pm4py.convert_to_petri_net(tree)
    candidates = [["start", x, y, "end"] for x, y in itertools.product("AB", "CD")]
    # Allow silent model transitions; visible insertions/deletions mean the trace is unsupported.
    replay = pm4py.conformance_diagnostics_alignments(log_of(candidates), net, im, fm)
    results[name] = {
        "training": seqs,
        "tree": str(tree),
        "accepted": {
            "".join(s[1:3]): all(
                left == right or (left == ">>" and right is None)
                for left, right in r["alignment"]
            )
            for s, r in zip(candidates, replay)
        },
    }
assert results["sparse_independent_or_constrained"]["accepted"]["AD"]
assert not results["correlated_only"]["accepted"]["AD"]
assert all(results["all_combinations"]["accepted"].values())
print(
    json.dumps(
        {"pm4py": importlib.metadata.version("pm4py"), "cases": results}, indent=2
    )
)

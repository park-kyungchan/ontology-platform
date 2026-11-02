#!/usr/bin/env python3
"""Real S06 old-reader roundtrip through S07 durable uncertainty. Lead build required."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import verify_v1 as v1
from verify_v2 import accepts, exact


def tables(root):
    with sqlite3.connect(root / "local.sqlite") as db:
        return {name: db.execute("select * from " + name + " order by 1").fetchall()
                for name in ["local_action_meta", "local_actions", "local_fences"]}


def run(binary, old_reader, results):
    v1.pins()
    results.mkdir(parents=True, exist_ok=False)
    cases = json.loads((v1.HERE / "cases-v1.json").read_text())["scenarios"]
    source_bytes = (v1.HERE / "source-v1.json").read_bytes()
    source = json.loads(source_bytes)["objects"][0]
    source["observations"][0]["lineage"] = {
        "artifact_sha256": hashlib.sha256(source_bytes).hexdigest(), "pointer": "/objects/0/observations/0"}
    report = {"kind": "actual_s06_binary_roundtrip", "binary": str(binary), "old_reader": str(old_reader),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "old_reader_sha256": hashlib.sha256(old_reader.read_bytes()).hexdigest(), "roundtrips": []}
    try:
        with tempfile.TemporaryDirectory(prefix="s07-old-reader-roundtrip-") as temp:
            for boundary in ["after_fence_commit", "after_target_commit"]:
                root = Path(temp) / boundary
                case = next(case for case in cases if case["id"] == "crash-" + boundary)
                events = []
                def invoke(step, index):
                    argv = [str(binary), "--root", str(root)]
                    if step["fault"]:
                        argv.extend(["--fault", step["fault"]])
                    child = subprocess.run(argv, input=json.dumps(step["command"]).encode(), capture_output=True, timeout=8)
                    tag = f"{boundary}-{index}"
                    (results / (tag + ".stdout")).write_bytes(child.stdout)
                    (results / (tag + ".stderr")).write_bytes(child.stderr)
                    events.append({"step": index, "status": child.returncode})
                    assert accepts(step, child.returncode, child.stdout, child.stderr), tag
                for i in range(3):
                    invoke(case["steps"][i], i)
                before = tables(root)
                sql_before, cells_before = v1.readback(root)
                old = subprocess.run([str(old_reader), str(root / "local.sqlite"), json.dumps(source["id"])],
                                     capture_output=True, timeout=8)
                (results / (boundary + "-old.stdout")).write_bytes(old.stdout)
                (results / (boundary + "-old.stderr")).write_bytes(old.stderr)
                assert old.returncode == 0 and not old.stderr
                assert exact(json.loads(old.stdout), source)
                assert tables(root) == before
                assert v1.readback(root) == (sql_before, cells_before)
                for i in [3, 5, 7, 8]:
                    invoke(case["steps"][i], i)
                sql_after, cells_after = v1.readback(root)
                assert sql_after == case["steps"][8]["sql"]
                report["roundtrips"].append({"boundary": boundary, "old_child_status": old.returncode,
                    "new_children": events, "side_state_unchanged_by_old_binary": True,
                    "before": sql_before, "after": sql_after, "cells_before": cells_before, "cells_after": cells_after})
        report["passed"] = True
    finally:
        (results / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": True, "actual_old_reader_children": 2, "roundtrips": 2}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--old-reader", type=Path, required=True)
    parser.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()
    run(args.binary.resolve(), args.old_reader.resolve(), args.results.resolve())

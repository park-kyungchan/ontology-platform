#!/usr/bin/env python3
"""Frozen literal process judge. No Rust output is used to create expectations."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent


def accepts(step, status, stdout, stderr):
    if status != step["status"] or stderr or len(stdout) > 65536:
        return False
    if step["output"] is None:
        return stdout == b""
    try:
        return json.loads(stdout) == step["output"] and stdout.endswith(b"\n")
    except (ValueError, UnicodeDecodeError):
        return False


def selfcheck():
    case = json.loads((HERE / "cases-v1.json").read_text())["scenarios"][0]["steps"][1]
    good = json.dumps(case["output"]).encode() + b"\n"
    assert accepts(case, 0, good, b"")
    mutants = [(2, good, b""), (0, good + b"junk", b""), (0, good, b"warning"),
               (0, b"{}\n", b""), (0, good.rstrip(), b"")]
    for path, value in [("state", "persisted"), ("desired", "invented"),
                        ("scope", {}), ("physical", {}), ("authorization_revision", 0)]:
        wrong = copy.deepcopy(case["output"])
        wrong[path] = value
        mutants.append((0, json.dumps(wrong).encode() + b"\n", b""))
    for args in mutants:
        assert not accepts(case, *args), args
    crash = dict(case, status=86, output=None)
    assert accepts(crash, 86, b"", b"")
    assert not accepts(crash, 0, b"", b"")
    assert not accepts(crash, 85, b"", b"")
    return {"positive_controls": 2, "negative_controls": len(mutants) + 2}


def pins():
    manifest = json.loads((HERE / "pins-v1.json").read_text())
    root = HERE.parent.parent
    for name, digest in manifest.items():
        assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name


def readback(root):
    with sqlite3.connect(root / "local.sqlite") as local:
        assert local.execute("pragma integrity_check").fetchone() == ("ok",)
        states = [row[0] for row in local.execute("select state from local_actions order by state")]
        fences = local.execute("select count(*) from local_fences").fetchone()[0]
    with sqlite3.connect(root / "target.sqlite") as target:
        assert target.execute("pragma integrity_check").fetchone() == ("ok",)
        effects = target.execute("select count(*) from dispatches where outcome='applied'").fetchone()[0]
        cells = list(target.execute("select instance,account,record,field,value,version from cells order by instance,account,record,field"))
    return {"states": states, "effects": effects, "fences": fences}, cells


def run(binary, results):
    pins()
    results.mkdir(parents=True, exist_ok=False)
    report = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "kind": "actual_rust_children", "selfcheck": selfcheck(), "steps": []}
    try:
        with tempfile.TemporaryDirectory(prefix="s07-fake-process-") as temp:
            for scenario in json.loads((HERE / "cases-v1.json").read_text())["scenarios"]:
                root = Path(temp) / scenario["id"]
                for index, step in enumerate(scenario["steps"]):
                    tag = f'{scenario["id"]}-{index:02d}'
                    argv = [str(binary), "--root", str(root), "--profile", step["profile"]]
                    if step["fault"]:
                        argv.extend(["--fault", step["fault"]])
                    child = subprocess.run(argv, input=json.dumps(step["command"]).encode(),
                                           capture_output=True, timeout=8, check=False)
                    (results / (tag + ".stdout")).write_bytes(child.stdout)
                    (results / (tag + ".stderr")).write_bytes(child.stderr)
                    item = {"id": tag, "argv": argv, "status": child.returncode,
                            "protocol_match": accepts(step, child.returncode, child.stdout, child.stderr)}
                    if step["sql"] is not None:
                        sql, cells = readback(root)
                        item.update(readback=sql, target_cells=cells, sql_match=sql == step["sql"])
                    report["steps"].append(item)
                    assert item["protocol_match"] and item.get("sql_match", True), tag
        report["passed"] = True
    finally:
        (results / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": True, "actual_child_count": len(report["steps"])}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--results", type=Path)
    args = parser.parse_args()
    if args.selfcheck:
        print(json.dumps(selfcheck()))
    else:
        assert args.binary and args.results
        run(args.binary.resolve(), args.results.resolve())

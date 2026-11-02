#!/usr/bin/env python3
"""Forward-only comparator correction; frozen v1 literal product cases unchanged."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import verify_v1 as original


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate key")
        result[key] = value
    return result


def exact(a, b):
    if type(a) is not type(b):
        return False
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(exact(a[k], b[k]) for k in a)
    if isinstance(a, list):
        return len(a) == len(b) and all(exact(x, y) for x, y in zip(a, b))
    return a == b


def accepts(step, status, stdout, stderr):
    if status != step["status"] or stderr or len(stdout) > 65536:
        return False
    if step["output"] is None:
        return stdout == b""
    try:
        value = json.loads(stdout, object_pairs_hook=no_duplicates)
        compact = json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"
        return stdout == compact and exact(value, step["output"])
    except (ValueError, UnicodeDecodeError):
        return False


base_pins = original.pins


def selfcheck():
    step = json.loads((original.HERE / "cases-v1.json").read_text())["scenarios"][0]["steps"][1]
    def compact(value):
        return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"
    good = compact(step["output"])
    assert accepts(step, 0, good, b"")
    mutants = [(2, good, b""), (0, good + b"junk", b""), (0, good, b"warning"),
               (0, b"{}\n", b""), (0, good.rstrip(), b""), (0, b" " + good, b""), (0, good + b"\n", b"")]
    for path, value in [("state", "persisted"), ("desired", "invented"),
                        ("scope", {}), ("physical", {}), ("authorization_revision", 0)]:
        wrong = copy.deepcopy(step["output"])
        wrong[path] = value
        mutants.append((0, compact(wrong), b""))
    for args in mutants:
        assert not accepts(step, *args), args
    crash = dict(step, output=None, status=86)
    assert accepts(crash, 86, b"", b"")
    assert not accepts(crash, 0, b"", b"")
    assert not accepts(crash, 85, b"", b"")
    boolean = copy.deepcopy(step["output"])
    boolean["authorization_revision"] = True
    assert not accepts(step, 0, compact(boolean), b"")
    duplicate = good.replace(b'"status":"ok"', b'"status":"refused","status":"ok"')
    assert not accepts(step, 0, duplicate, b"")
    child = subprocess.run([sys.executable, "-c", "import sys;print(sys.argv[1]);sys.exit(7)", good.decode().rstrip("\n")], capture_output=True, timeout=5)
    assert not accepts(step, child.returncode, child.stdout, child.stderr)
    killed = subprocess.run([sys.executable, "-c", "import os;os._exit(85)"], capture_output=True, timeout=5)
    assert not accepts(crash, killed.returncode, killed.stdout, killed.stderr)
    return {"positive_controls": 2, "negative_controls": len(mutants) + 6,
            "actual_child_statuses": [child.returncode, killed.returncode], "rust_candidate_executed": False}


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--results", type=Path)
    args = parser.parse_args()
    original.accepts = accepts
    original.selfcheck = selfcheck
    # Retain native v1 pin check, then additionally check this correction's pins.
    def all_pins():
        base_pins()
        root = original.HERE.parent.parent
        for name, digest in json.loads((original.HERE / "pins-v2.json").read_text()).items():
            assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name
    original.pins = all_pins
    if args.selfcheck:
        all_pins()
        print(json.dumps(selfcheck()))
    else:
        assert args.binary and args.results
        original.run(args.binary.resolve(), args.results.resolve())

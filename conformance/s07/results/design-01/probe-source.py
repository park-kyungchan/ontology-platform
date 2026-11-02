#!/usr/bin/env python3
"""Finite SQLite DESIGN probe, not Rust execution or an implementation oracle.

Exercises actual child process exits/SQLite recovery with the candidate SQL schema.
Only newly created temporary synthetic databases are touched.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
BOUNDARIES = {
    "before_review_commit": (81, None, 0), "after_review_commit": (82, "reviewed", 0),
    "before_fence_commit": (83, "reviewed", 0), "after_fence_commit": (84, "possible_dispatch", 0),
    "before_target_commit": (85, "possible_dispatch", 0), "after_target_commit": (86, "possible_dispatch", 1),
    "before_readback": (89, "possible_dispatch", 1), "before_receipt_commit": (87, "possible_dispatch", 1),
    "after_receipt_commit": (88, "persisted", 1),
}


def connect(path):
    conn = sqlite3.connect(path, timeout=.25, isolation_level=None)
    conn.execute("pragma foreign_keys=on")
    conn.execute("pragma synchronous=full")
    conn.execute("pragma journal_mode=delete")
    conn.execute("pragma trusted_schema=off")
    return conn


def initialize(root):
    root.mkdir()
    with connect(root / "local.sqlite") as local:
        local.executescript((ROOT / "crates/ont-store/schema.sql").read_text())
        local.executescript((ROOT / "crates/ont-store/actions.sql").read_text())
        local.execute("insert into objects values('progress','synthetic://s07','A','occurrence-1','{}','v1')")
        journal = local.execute("select journal_id from local_action_meta").fetchone()[0]
    with connect(root / "target.sqlite") as target:
        target.executescript((ROOT / "crates/ont-store/fake.sql").read_text())
        target.execute("insert into meta values(1,'design-probe-target',?)", (journal,))
        target.execute("insert into cells values('synthetic://s07','A','R-001','note','{}','old',0)")


def review(local):
    local.execute("insert into local_actions values('token','synthetic://s07','A','r1','progress','occurrence-1','{}','reviewed','old',null)")


def worker(root, boundary):
    def crash(name):
        if name == boundary:
            os._exit(BOUNDARIES[name][0])
    local = connect(root / "local.sqlite")
    target = connect(root / "target.sqlite")
    if "review_commit" in boundary:
        local.execute("begin immediate")
        target.execute("begin immediate")
        review(local)
        crash("before_review_commit")
        local.execute("commit")
        crash("after_review_commit")
    local.execute("begin immediate")
    target.execute("begin immediate")
    local.execute("insert into local_fences values('synthetic://s07','A','R-001','note','token')")
    local.execute("update local_actions set state='possible_dispatch',observed=null")
    crash("before_fence_commit")
    local.execute("commit")
    crash("after_fence_commit")
    target.execute("update cells set value='practice fractions',version=1")
    target.execute("insert into dispatches values('token','{}','applied',1,'practice fractions')")
    crash("before_target_commit")
    target.execute("commit")
    crash("after_target_commit")
    crash("before_readback")
    local.execute("begin immediate")
    target.execute("begin immediate")
    assert target.execute("select value,version from cells").fetchone() == ("practice fractions", 1)
    target.execute("commit")
    local.execute("update local_actions set state='persisted',observed='practice fractions',proof='design-probe-applied'")
    local.execute("delete from local_fences")
    crash("before_receipt_commit")
    local.execute("commit")
    crash("after_receipt_commit")
    raise AssertionError("unreached crash boundary")


def readback(root):
    with connect(root / "local.sqlite") as local, connect(root / "target.sqlite") as target:
        return {"states": [r[0] for r in local.execute("select state from local_actions")],
                "fences": local.execute("select count(*) from local_fences").fetchone()[0],
                "cell": list(target.execute("select value,version from cells").fetchone()),
                "dispatches": list(target.execute("select outcome,version,value from dispatches")),
                "local_integrity": local.execute("pragma integrity_check").fetchone()[0],
                "target_integrity": target.execute("pragma integrity_check").fetchone()[0]}


def reconcile(root):
    with connect(root / "local.sqlite") as local, connect(root / "target.sqlite") as target:
        local.execute("begin immediate")
        target.execute("begin immediate")
        proof = target.execute("select outcome from dispatches where token='token'").fetchone()
        value, version = target.execute("select value,version from cells").fetchone()
        if proof is None:
            target.execute("insert into dispatches values('token','{}','sealed_no_effect',?,?)", (version, value))
        state = "persisted" if proof else "no_effect"
        target.execute("commit")
        local.execute("update local_actions set state=?,observed=?,proof='design-probe-durable'", (state, value))
        local.execute("delete from local_fences")
        local.execute("commit")
        return state


def run(results):
    results.mkdir(parents=True, exist_ok=False)
    report = {"kind": "python_sqlite_design_only", "rust_executed": False, "children": []}
    try:
        with tempfile.TemporaryDirectory(prefix="s07-sqlite-design-") as temp:
            for boundary, (status, state, effects) in BOUNDARIES.items():
                root = Path(temp) / boundary
                initialize(root)
                if "review_commit" not in boundary:
                    with connect(root / "local.sqlite") as local:
                        review(local)
                child = subprocess.run([sys.executable, str(Path(__file__).resolve()), "--child", str(root), boundary],
                                       capture_output=True, timeout=8)
                (results / (boundary + ".stdout")).write_bytes(child.stdout)
                (results / (boundary + ".stderr")).write_bytes(child.stderr)
                before = readback(root)
                assert child.returncode == status and not child.stdout and not child.stderr
                assert before["states"] == ([] if state is None else [state])
                assert before["fences"] == (1 if state == "possible_dispatch" else 0)
                assert sum(r[0] == "applied" for r in before["dispatches"]) == effects
                assert before["cell"] == (["practice fractions", 1] if effects else ["old", 0])
                item = {"boundary": boundary, "status": child.returncode, "readback": before}
                # S06 configure/read SQL compatibility, not an old binary claim.
                with connect(root / "local.sqlite") as old:
                    assert old.execute("pragma application_id").fetchone() == (1330533430,)
                    assert old.execute("pragma user_version").fetchone() == (1,)
                    assert old.execute("select scope,head from objects where object_type=? and instance=? and account=? and native_key=?",
                                       ("progress", "synthetic://s07", "A", "occurrence-1")).fetchone() == ("{}", "v1")
                    if state == "possible_dispatch":
                        try:
                            old.execute("update objects set head='v2'")
                            raise AssertionError("pending basis was writable")
                        except sqlite3.IntegrityError as error:
                            assert str(error) == "local_action_basis_held"
                            item["basis_update_status"] = "constraint_abort"
                if state == "possible_dispatch":
                    # Merely reading an old/desired value leaves the fence intact.
                    assert readback(root) == before
                    item["reconciled"] = reconcile(root)
                    item["after_reconcile"] = readback(root)
                    assert item["reconciled"] == ("persisted" if effects else "no_effect")
                    assert item["after_reconcile"]["fences"] == 0
                report["children"].append(item)
        report["passed"] = True
    finally:
        report["schema_sha256"] = {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
                                  for name in ["crates/ont-store/schema.sql", "crates/ont-store/actions.sql", "crates/ont-store/fake.sql"]}
        (results / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"kind": report["kind"], "passed": True, "actual_python_crash_statuses": [c["status"] for c in report["children"]]}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--child", nargs=2)
    parser.add_argument("--results", type=Path)
    args = parser.parse_args()
    if args.child:
        worker(Path(args.child[0]), args.child[1])
    else:
        assert args.results
        run(args.results.resolve())

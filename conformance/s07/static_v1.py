#!/usr/bin/env python3
"""Permitted static checks only: no compiler, Rust candidate, network or real DB."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def run(out):
    out.mkdir(parents=True, exist_ok=False)
    commands = [
        ["cargo", "+1.95.0", "fmt", "--all", "--", "--check"],
        ["cargo", "+1.95.0", "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1"],
        ["cargo", "+1.95.0", "fetch", "--locked", "--offline"],
        [sys.executable, "conformance/verify.py", "--self-check"],
        [sys.executable, "conformance/s07/verify_v2.py", "--selfcheck"],
        [sys.executable, "-m", "py_compile", *[str(p.relative_to(ROOT)) for p in sorted(HERE.glob("*.py"))]],
        ["rustfmt", "+1.95.0", "--edition", "2024", "--check", "conformance/s07/old_reader.rs"],
        ["git", "diff", "--check"],
        ["git", "diff", "--cached", "--check"],
    ]
    report = {"kind": "static_only", "rust_compiled": False, "rust_candidate_executed": False, "commands": []}
    try:
        for index, argv in enumerate(commands):
            child = subprocess.run(argv, cwd=ROOT, capture_output=True, timeout=45)
            (out / f"{index:02d}.stdout").write_bytes(child.stdout)
            (out / f"{index:02d}.stderr").write_bytes(child.stderr)
            report["commands"].append({"argv": argv, "status": child.returncode})
            assert child.returncode == 0, argv
        for name in ["pins-v1.json", "pins-v2.json"]:
            for path, digest in json.loads((HERE / name).read_text()).items():
                assert hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == digest, path
        source_paths = set(ROOT.glob("Cargo*")) | set(ROOT.glob("README.md"))
        source_paths |= {p for p in (ROOT / "crates").rglob("*") if p.suffix in {".rs", ".toml", ".sql", ".md"}}
        source_paths |= set((ROOT / "docs/s07").glob("*.md"))
        source_paths |= {p for p in HERE.iterdir() if p.suffix in {".py", ".rs", ".json"}}
        report["source_sha256"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                   for path in sorted(source_paths) if path.is_file()}
        report["passed"] = True
    finally:
        (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"kind": "static_only", "passed": True, "actual_statuses": [row["status"] for row in report["commands"]]}))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()
    run(args.results.resolve())

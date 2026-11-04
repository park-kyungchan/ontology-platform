#!/usr/bin/env python3
"""Lead-only preparation: pristine accepted S06 archive plus read-only helper.

Does not compile, execute a candidate, or touch an existing destination.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tarfile

BASE = "224864887d19f2c52e5026568f5e9b169314d3be"
HERE = Path(__file__).resolve().parent

if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    dest = args.out.resolve()
    dest.mkdir(parents=True, exist_ok=False)
    archive = dest / "accepted-s06.tar"
    subprocess.run(["git", "archive", "--format=tar", "--output", str(archive), BASE],
                   cwd=HERE.parent.parent, check=True, timeout=30)
    source = dest / "source"
    source.mkdir()
    with tarfile.open(archive) as contents:
        contents.extractall(source, filter="data")
    helper = source / "crates/ontd/src/bin/s07-old-reader.rs"
    helper.parent.mkdir(exist_ok=True)
    shutil.copyfile(HERE / "old_reader.rs", helper)
    (dest / "PREPARED.json").write_text(json.dumps({"base": BASE, "source": str(source),
        "only_added_file": str(helper.relative_to(source)), "compiled": False}, indent=2) + "\n")
    print(json.dumps({"base": BASE, "source": str(source), "compiled": False}))

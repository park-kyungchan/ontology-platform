#!/usr/bin/env python3
"""Probe format-4 rejection using real pinned-old and candidate binaries."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import sqlite3
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--old-binary", required=True)
parser.add_argument("--new-binary", required=True)
parser.add_argument("--directory", type=Path, required=True)
args = parser.parse_args()
args.directory.mkdir(mode=0o700)

def run(binary, store, command, body=None):
    return subprocess.run([binary,"--store",str(store),"--now","1",*command], input=json.dumps(body or {}),text=True,capture_output=True)

def success(binary, store, command, body=None):
    result = run(binary,store,command,body)
    assert result.returncode == 0, (command,result.stdout,result.stderr)
    return result.stdout

def version(store):
    with sqlite3.connect("file:"+str(store.resolve())+"?mode=ro",uri=True) as connection:
        return connection.execute("PRAGMA user_version").fetchone()[0]

def action(key, status, request):
    return {"action_type":"upsert","parameters":{"spec":{"key":key},"status":status},"request_id":request}

def drop(key, request):
    return {"action_type":"drop","parameters":{"record":{"object_type":"record","namespace":{"instance":"test","account":"root"},"native_key":key}},"request_id":request}

schema = {"format":"ontology-schema/v1","domain":"barrier","version":1,"namespace":{"instance":"test","account":"root"},"scope_keys":[],"sources":{},"value_types":{},
    "object_types":{"record":{"owner":"ontology","primary_key":{"from":"/key"},"properties":{"status":{"type":"string"}}}},"link_types":{},"functions":{},
    "action_types":{"upsert":{"parameters":{"spec":{"type":{"struct":{"key":"string"}},"required":True},"status":{"type":"string","required":True}},"rules":[{"create_or_modify_object":"record","set":{"status":"$status"}}]},
        "drop":{"parameters":{"record":{"type":{"ref":"record"},"required":True}},"rules":[{"delete_object":"record"}]}},
    "grants":[{"principal":"agent","scope":{},"read":["record"],"derive":[],"actions":["upsert","drop"]}]}
path = args.directory / "schema.json"
path.write_text(json.dumps(schema))
seed = args.directory / "seed"
seed.mkdir()
store = seed / "store.sqlite"
assert "50dd391239a880dcbabbe775321c0355588c9de1" in success(args.old_binary,store,["--version"])
success(args.old_binary,store,["schema","install",str(path)])
success(args.old_binary,store,["action","apply"],action("target","superseded","seed-target"))
success(args.old_binary,store,["action","apply"],action("deleted","ready","seed-deleted"))
success(args.old_binary,store,["action","apply"],drop("deleted","seed-drop"))
backup = args.directory / "before.sqlite"
shutil.copy2(store,backup)
assert version(backup)==3
success(args.new_binary,store,["describe"])
assert version(store)==4
schema["version"] = 2
upgrade = args.directory / "upgrade.json"
upgrade.write_text(json.dumps(schema))
cases = [("create",["action","apply"],action("new","ready","probe-create")),
    ("change",["action","apply"],action("target","ready","probe-change")),
    ("retract",["action","apply"],drop("target","probe-retract")),
    ("resurrect",["action","apply"],action("deleted","ready","probe-resurrect")),
    ("schema_install",["schema","install",str(upgrade)],None), ("read_only",["describe"],None)]
results = []
for name, command, body in cases:
    control = args.directory / name
    control.mkdir()
    old = control / "store.sqlite"
    shutil.copy2(backup,old)
    success(args.old_binary,old,command,body)  # Every attempt is valid on format 3.
    before = hashlib.sha256(store.read_bytes()).hexdigest()
    result = run(args.old_binary,store,command,body)
    assert result.returncode==1 and not result.stdout and "store_failure@" in result.stderr
    assert hashlib.sha256(store.read_bytes()).hexdigest()==before
    results.append({"case":name,"exit":result.returncode,"stdout":result.stdout,"stderr":result.stderr.strip()})
restored = args.directory / "restored"
restored.mkdir()
recovery = restored / "store.sqlite"
shutil.copy2(backup,recovery)
assert version(recovery)==3
success(args.new_binary,recovery,["describe"])
assert version(recovery)==4
request = {"object_type":"record","where":{"eq":["status","ready"]}}
assert success(args.new_binary,recovery,["search"],request)==success(args.new_binary,store,["search"],request)
print(json.dumps(results,indent=2))

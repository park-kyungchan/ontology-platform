#!/usr/bin/env python3
"""Reproduce the synthetic format-3 fixture using the pinned old binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess

PIN = "50dd391239a880dcbabbe775321c0355588c9de1"
SCHEMA = '{"format": "ontology-schema/v1", "domain": "previous", "version": 1, "namespace": {"instance": "test", "account": "root"}, "scope_keys": [], "value_types": {}, "sources": {}, "object_types": {"record": {"owner": "ontology", "primary_key": {"from": "$action_id"}, "properties": {"status": {"type": "string"}}}}, "link_types": {}, "action_types": {"create_record": {"parameters": {"status": {"type": "string", "required": true}}, "rules": [{"create_object": "record", "set": {"status": "$status"}}]}}, "functions": {}, "grants": [{"principal": "agent", "scope": {}, "read": ["record"], "derive": [], "actions": ["create_record"]}]}'
EXPECTED = {'objects': 'be0df148499ace85644c9c2861f979433e2bfda71c4f1e7e59373e7f8fbba4e2', 'observations': '7ae8c6c1d58fea8b5abc4e82b9c514437b89abc5c49e0cf86b82658e65962767', 'schema_installs': '27a63d47cff2bc89289567d492297bc6d58fabaaf0adfcca001933bae37310f2', 'action_log': '3e63dcbf0f205cc87dcae8a7732f4062a9713a730f0d0b267e00d07cf070ddcc'}

def logical(connection):
    out = {}
    for table in EXPECTED:
        rows = connection.execute("SELECT * FROM " + table).fetchall()
        rows = sorted([[{"hex":v.hex()} if isinstance(v, bytes) else v for v in row] for row in rows], key=lambda r:json.dumps(r,sort_keys=True))
        out[table] = hashlib.sha256(json.dumps(rows,sort_keys=True,separators=(",",":"),ensure_ascii=False).encode()).hexdigest()
    return out

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--old-binary", required=True)
    parser.add_argument("--directory", type=Path, required=True)
    args = parser.parse_args()
    args.directory.mkdir(mode=0o700)
    store = args.directory / "store.sqlite"
    env = dict(os.environ)
    env.pop("ONT_AGENT", None)
    env.pop("ONT_SESSION", None)
    def run(command, body="", now="1"):
        return subprocess.run([args.old_binary,"--store",str(store),"--now",now,*command], input=body,text=True,env=env,capture_output=True,check=True).stdout
    assert PIN in run(["--version"])
    schema = args.directory / "schema.json"
    schema.write_text(SCHEMA)
    run(["schema","install",str(schema)])
    for status in ["ready","superseded"]:
        run(["action","apply"],json.dumps({"action_type":"create_record","parameters":{"status":status},"request_id":"old-"+status}))
    with sqlite3.connect("file:"+str(store.resolve())+"?mode=ro",uri=True) as connection:
        actual = logical(connection)
    assert actual == EXPECTED, (actual, EXPECTED)
    answer = run(["search"],json.dumps({"object_type":"record","where":{"eq":["status","ready"]},"limit":1000}),"10")
    (args.directory / "answer.json").write_text(answer)
    print(json.dumps(actual,sort_keys=True))

if __name__ == "__main__":
    main()

"""Finite Python stdlib SQLite experiment; never opens a caller-supplied/live path.
Not a rusqlite test or action-recovery experiment. Result log survives temp cleanup.
"""
import json, sqlite3, subprocess, sys, tempfile, threading
from pathlib import Path
P=Path(__file__).resolve().parent
SCHEMA=(P.parent/'crates/ont-store/schema.sql').read_text()

def main():
 results={}
 with tempfile.TemporaryDirectory(prefix='s06-synthetic-') as tmp:
  path=Path(tmp)/'synthetic.sqlite'
  c=sqlite3.connect(path,timeout=0.1)
  c.execute('PRAGMA foreign_keys=ON'); c.execute('PRAGMA synchronous=FULL')
  c.executescript(SCHEMA)
  ids=[('student','host','A','001'),('student','host','B','001'),('student','host','A','1'),('student','host','A','a|b'),('student|a','host','A','b')]
  for i in ids: c.execute('INSERT INTO objects VALUES (?,?,?,?,?,?)',(*i,'{}','r1'))
  c.commit()
  assert c.execute('SELECT count(*) FROM objects').fetchone()==(5,)
  results['composite_identity']='5 distinct exact identities'
  c.execute('BEGIN IMMEDIATE')
  c.execute("UPDATE objects SET head='rolled-back' WHERE account='A'")
  c.rollback()
  assert c.execute("SELECT DISTINCT head FROM objects").fetchall()==[('r1',)]
  results['explicit_rollback']='old heads preserved'
  # Read snapshot holds a SHARED lock in DELETE journal. A second writer can
  # prepare a change but COMMIT waits/refuses until reader exits, bounded here.
  c.execute('BEGIN'); c.execute('SELECT head FROM objects').fetchall()
  writer=sqlite3.connect(path,timeout=0.05)
  writer.execute('BEGIN IMMEDIATE'); writer.execute("UPDATE objects SET head='r2' WHERE account='A'")
  try: writer.commit(); raise AssertionError('writer committed across active reader')
  except sqlite3.OperationalError as e:
   assert str(e)=='database is locked'
  assert c.execute('SELECT DISTINCT head FROM objects').fetchall()==[('r1',)]
  c.commit(); writer.commit(); writer.close()
  assert set(c.execute('SELECT DISTINCT head FROM objects').fetchall())=={('r1',),('r2',)}
  results['snapshot']='coherent old heads; bounded writer commit lock; new heads after reader commit'
  c.execute('BEGIN IMMEDIATE')
  blocked=sqlite3.connect(path,timeout=0.05)
  try: blocked.execute('BEGIN IMMEDIATE'); raise AssertionError('second writer acquired')
  except sqlite3.OperationalError as e: assert str(e)=='database is locked'
  blocked.close(); c.rollback()
  results['writer_serialization']='second writer refused within 50ms timeout'
  # Child crash with an uncommitted local SQLite edit; no external effect.
  program="""import os,sqlite3,sys
c=sqlite3.connect(sys.argv[1]); c.execute('PRAGMA synchronous=FULL'); c.execute('BEGIN IMMEDIATE'); c.execute(\"UPDATE objects SET head='crashed'\"); os._exit(23)
"""
  child=subprocess.run([sys.executable,'-c',program,str(path)],capture_output=True,timeout=5)
  assert child.returncode==23 and child.stdout==b'' and child.stderr==b''
  c.close(); c=sqlite3.connect(path)
  assert 'crashed' not in [r[0] for r in c.execute('SELECT head FROM objects')]
  assert c.execute('PRAGMA integrity_check').fetchall()==[('ok',)]
  results['uncommitted_child_exit']={'actual_exit':23,'reopen':'pre-crash committed values only','integrity_check':'ok'}
  c.close()
 return {'kind':'python_stdlib_not_rusqlite','sqlite_version':sqlite3.sqlite_version,'checks':results,'live_paths_opened':0,'rust_compilation':False}
if __name__=='__main__':
 result=main(); (P/'results/sqlite-python.json').write_text(json.dumps(result,indent=2)+'\n'); print(json.dumps(result,indent=2))

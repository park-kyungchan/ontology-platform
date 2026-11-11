"""No compiler or candidate invocation. Preserve exact command status and output."""
import argparse,copy,hashlib,json,subprocess,sys,tomllib
from pathlib import Path
P=Path(__file__).resolve().parent; ROOT=P.parent

def exact_json(a,b):
 if type(a) is not type(b):return False
 if isinstance(a,dict):return a.keys()==b.keys() and all(exact_json(a[k],b[k]) for k in a)
 if isinstance(a,list):return len(a)==len(b) and all(exact_json(x,y) for x,y in zip(a,b))
 return a==b

def main():
 parser=argparse.ArgumentParser(); parser.add_argument("--out",type=Path,required=True); args=parser.parse_args()
 out=args.out.resolve(); out.mkdir(parents=True,exist_ok=False)
 rows=[]
 commands=[['cargo','+1.95.0','fmt','--all','--','--check'],['cargo','+1.95.0','metadata','--locked','--offline','--no-deps','--format-version','1'],[sys.executable,str(P/'verify.py'),'--self-check'],['git','diff','--cached','--check'],['git','diff','--check']]
 for i,cmd in enumerate(commands):
  child=subprocess.run(cmd,cwd=ROOT,capture_output=True,text=True,timeout=30)
  (out/f'static-{i:02d}.stdout').write_text(child.stdout)
  (out/f'static-{i:02d}.stderr').write_text(child.stderr)
  rows.append({'argv':cmd,'exit':child.returncode,'stdout':f'static-{i:02d}.stdout','stderr':f'static-{i:02d}.stderr'})
  assert child.returncode==0,rows[-1]
 meta=json.loads((out/'static-01.stdout').read_text()); assert {x['name'] for x in meta['packages']}=={'ont-core','ont-store','ont-daemon','ont'}
 lock=tomllib.loads((ROOT/'Cargo.lock').read_text())
 expected={'rusqlite':'0.32.1','libsqlite3-sys':'0.30.1','serde':'1.0.228','serde_json':'1.0.145','sha2':'0.10.9'}
 for name,version in expected.items(): assert any(x['name']==name and x['version']==version for x in lock['package'])
 pins=json.loads((P/'contract-pins-v2.json').read_text())
 for f,h in pins.items(): assert hashlib.sha256((ROOT/f).read_bytes()).hexdigest()==h
 assert hashlib.sha256((P/'fixtures/s05/expected.json').read_bytes()).hexdigest()=='aae41a0f4d79752a0b14d2d4504d5112d4bc576ba6827412675f6789eb6ee234'
 frames=json.loads((P/'fixtures/s05/expected.json').read_text()); audit=json.loads((P/'fixtures/s05/audit.json').read_text())
 count=0
 for case in ['sections-edition-a','attempt-knowledge-states']:
  for row in frames[case]['value']['records']:
   v=audit
   for key in row['source']['jsonPointer'].lstrip('/').split('/'):
    key=key.replace('~1','/').replace('~0','~'); v=v[int(key)] if isinstance(v,list) else v[key]
   assert v==row['value']; count+=1
 source=json.loads((P/'fixtures/synthetic-source.json').read_text()); fixture=json.loads((P/'fixtures/synthetic-v2.json').read_text())
 source_digest=hashlib.sha256((P/'fixtures/synthetic-source.json').read_bytes()).hexdigest()
 stripped=copy.deepcopy(fixture)
 for i,obj in enumerate(stripped['objects']):
  for j,ob in enumerate(obj['observations']):
   lineage=ob.pop('lineage'); assert lineage=={'artifact_sha256':source_digest,'pointer':f'/objects/{i}/observations/{j}'}
 assert exact_json(stripped,source)
 v1=json.loads((P/'cases.json').read_text());v2=json.loads((P/'cases-v2.json').read_text())
 def clear_synthetic_lineage(v):
  if isinstance(v,dict):
   if v.get('artifact_sha256') in ('0'*64,source_digest): v['artifact_sha256']='synthetic'; v['pointer']='synthetic'
   for x in v.values():clear_synthetic_lineage(x)
  elif isinstance(v,list):
   for x in v:clear_synthetic_lineage(x)
 clear_synthetic_lineage(v1);clear_synthetic_lineage(v2);assert exact_json(v1,v2)
 report={'commands':rows,'frozen_pins':len(pins),'packages':len(meta['packages']),'s05_exact_audit_pointer_rows':count,'v2_changes_only_verified_synthetic_lineage':True,'rust_compilation':False,'candidate_execution':False,'lockfile_sha256':hashlib.sha256((ROOT/'Cargo.lock').read_bytes()).hexdigest()}
 (out/'static-report.json').write_text(json.dumps(report,indent=2)+'\n'); print(json.dumps(report,indent=2))
if __name__=='__main__':main()

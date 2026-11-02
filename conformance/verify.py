"""Independent literal comparator. Never constructs expected output from candidate.
Usage: --self-check (no Rust), or --candidate ABSOLUTE_BINARY --out NEW_DIRECTORY.
"""
import argparse, copy, hashlib, json, subprocess, sys
from pathlib import Path
P=Path(__file__).resolve().parent

def no_duplicates(pairs):
 d={}
 for k,v in pairs:
  if k in d: raise ValueError('duplicate object key')
  d[k]=v
 return d

def exact(a,b):
 if type(a) is not type(b): return False
 if isinstance(a,dict): return a.keys()==b.keys() and all(exact(a[k],b[k]) for k in a)
 if isinstance(a,list): return len(a)==len(b) and all(exact(x,y) for x,y in zip(a,b))
 return a==b

def compact(v): return json.dumps(v,ensure_ascii=False,separators=(',',':'))+'\n'

def judge(case, actual):
 if actual['exit']!=case['exit'] or actual['stderr']!='' or actual.get('timeout'): return False
 raw=actual['stdout']
 if len(raw.encode())>2*1024*1024: return False
 try:
  value=json.loads(raw,object_pairs_hook=no_duplicates)
  return raw==compact(value) and exact(value,case['expected'])
 except (ValueError,TypeError): return False

def pin_check():
 pins=json.loads((P/'contract-pins-v2.json').read_text())
 for path,digest in pins.items():
  assert hashlib.sha256((P.parent/path).read_bytes()).hexdigest()==digest,path
 return len(pins)

def self_check(cases):
 case=cases[0]; good={'exit':0,'stderr':'','stdout':compact(case['expected'])}
 assert judge(case,good)
 mutants=[]
 for patch in [{'exit':7},{'stderr':'junk\n'},{'stdout':good['stdout']+'junk\n'},{'stdout':' '+good['stdout']},{'stdout':'{}\n'},{'stdout':'{"status":"ok","status":"refused"}\n'}]:
  mutants.append(dict(good,**patch))
 for mutate in ['wrong-key','missing','extra','boolean-as-zero','stale','knowledge']:
  value=copy.deepcopy(case['expected']); view=value['result']['objects'][0]
  if mutate=='wrong-key': view['id']['native_key']='1'
  if mutate=='missing': del view['scope']
  if mutate=='extra': value['hidden_count']=1
  if mutate=='boolean-as-zero': view['observations'][0]['stale']=0
  if mutate=='stale': view['observations'][0]['stale']=True
  if mutate=='knowledge': view['observations'][0]['fields']['label']={'kind':'unknown'}
  mutants.append(dict(good,stdout=compact(value)))
 multi=next(c for c in cases if c['id']=='ONT-02-traverse')
 wrong=copy.deepcopy(multi['expected']); wrong['result']['objects'].reverse()
 assert not judge(multi,dict(good,stdout=compact(wrong)))
 for mutant in mutants: assert not judge(case,mutant)
 # Real child status is also inspected; output alone cannot supply success.
 child=subprocess.run([sys.executable,'-c','import sys; print(sys.argv[1]); sys.exit(7)',good['stdout'].rstrip('\n')],capture_output=True,text=True,timeout=5)
 assert not judge(case,{'exit':child.returncode,'stdout':child.stdout,'stderr':child.stderr})
 return {'pins_verified':pin_check(),'literal_cases':len(cases),'negative_controls_rejected':len(mutants)+2,'real_child_exit':child.returncode,'candidate_executed':False}

def main():
 parser=argparse.ArgumentParser(); parser.add_argument('--self-check',action='store_true'); parser.add_argument('--candidate',type=Path); parser.add_argument('--out',type=Path)
 args=parser.parse_args(); cases=json.loads((P/'cases-v2.json').read_text()); pin_check()
 if args.self_check:
  report=self_check(cases); print(json.dumps(report,indent=2)); return
 if not args.candidate or not args.out: parser.error('--candidate and --out required')
 args.out.mkdir(parents=True,exist_ok=False); rows=[]
 for n,case in enumerate(cases):
  try:
   child=subprocess.run([str(args.candidate.resolve()),'--synthetic'],input=compact(case['input']),capture_output=True,text=True,timeout=5)
   actual={'exit':child.returncode,'stdout':child.stdout,'stderr':child.stderr,'timeout':False}
  except subprocess.TimeoutExpired as e:
   actual={'exit':None,'stdout':str(e.stdout),'stderr':str(e.stderr),'timeout':True}
  passed=judge(case,actual); rows.append({'id':case['id'],'passed':passed})
  (args.out/f'{n:02d}.json').write_text(json.dumps({'id':case['id'],'actual':actual,'passed':passed},ensure_ascii=False,indent=2)+'\n')
 report={'candidate':str(args.candidate.resolve()),'binary_sha256':hashlib.sha256(args.candidate.read_bytes()).hexdigest(),'checks':rows,'all_passed':all(x['passed'] for x in rows)}
 (args.out/'report.json').write_text(json.dumps(report,indent=2)+'\n'); print(json.dumps(report,indent=2))
 sys.exit(0 if report['all_passed'] else 1)
if __name__=='__main__': main()

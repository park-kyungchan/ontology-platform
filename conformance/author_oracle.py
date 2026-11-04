"""One-time preimplementation oracle authoring. No candidate import or invocation.
Retained for provenance; frozen JSON, not regeneration, is the comparison authority.
"""
import json
from pathlib import Path
from datetime import datetime
P=Path(__file__).parent
NOW=1793577600
NS={'instance':'synthetic://same-host','account':'A'}
REC={'instance':'recording://s05-synthetic','account':'fixture-A'}
def ident(t,k,ns=NS): return {'object_type':t,'namespace':ns,'native_key':k}
def scope(ns=NS,student='001',klass='C1'): return {'namespace':ns,'class_key':klass,'student_key':student}
def obs(rev,fields,coverage='complete',change='initial',sup=None,at=1793577500,until=1793577800,src=None,pointer='/synthetic',digest='0'*64):
 return dict(revision=rev,source_revision=src or rev,observed_at=at,fresh_until=until,change=change,supersedes=sup,coverage=coverage,fields=fields,lineage={'artifact_sha256':digest,'pointer':pointer})
def value(x): return {'kind':'value','value':x}
def view(i,observations,sc=None):
 return dict(id=i,scope=sc or scope(i['namespace']),head=observations[-1]['revision'],observations=[dict(o,stale=NOW>o['fresh_until']) for o in observations])
student=ident('student','001'); book=ident('textbook','book:01|x'); progress=ident('progress','0007|x')
s=obs('s1',{'label':value('Synthetic Student')})
b=obs('b1',{'title':value('Synthetic Book')})
p1=obs('p1',{'score':value(0)},'partial',at=1793577000,until=1793577100)
p2=obs('p2',{'score':{'kind':'unknown'}},'partial','correction','p1')
p3=obs('p3',{'score':{'kind':'missing'}},'partial','backfill','p2',at=1793576000,until=1793576100)
objects=[dict(id=student,scope=scope(),observations=[s]),dict(id=book,scope=scope(),observations=[b]),dict(id=progress,scope=scope(),observations=[p1,p2,p3])]
# Collision and authorization traps. None is resolved by label.
for i,sc in [(ident('student','001',{'instance':NS['instance'],'account':'B'}),scope({'instance':NS['instance'],'account':'B'})),(ident('student','1'),scope()),(ident('student','001 '),scope()),(ident('progress','secret'),scope(student='002')),(ident('progress','class-secret'),scope(klass='C2')),(ident('student','a|b'),scope()),(ident('student|a','b'),scope())]:
 objects.append(dict(id=i,scope=sc,observations=[obs('x1',{'label':value('Synthetic Student')})]))
links=[dict(source=student,relation='learning',target=o['id']) for o in objects[1:6]]
links += [dict(source=student,relation='learning',target=ident('progress','class-secret')),dict(source=student,relation='books',target=book)]
# Explicit fixed recorded namespace: no inference of native account from teacher label.
expected=json.loads((P/'fixtures/s05/expected.json').read_text()); audit=json.loads((P/'fixtures/s05/audit.json').read_text())
recorded_root=ident('student','synthetic-student-1',REC)
recorded_obs=obs('saved-s05',{'teacher_authenticated':value(False),'complete_roster_verified':value(False),'absence_proves_nonperformance':value(False)},'partial',at=1791964800,until=1791964800,src='9b5a3c565f4aa9296ba98cdd7460138aa0642f38ded31470e9c8327cda12ee9c',pointer='/students/0',digest='4bdf5431cf2823cf707f9b73d2f7b07b47122369fde7ba7a0831bd9700922c3e')
recorded_views=[view(recorded_root,[recorded_obs],scope(REC,'synthetic-student-1','saved-subset'))]
for case,t,facet in [('sections-edition-a','section','sections'),('attempt-knowledge-states','attempt','attempts')]:
 for row in expected[case]['value']['records']:
  raw=row['value']; fields={'source_record':value(raw)}
  if facet=='attempts':
   fields['score']=({'kind':'not_performed'} if raw.get('assessmentState')=='not_performed' else {'kind':'missing'} if 'score' not in raw else {'kind':'null'} if raw['score'] is None else {'kind':'blank'} if raw['score']=='' else {'kind':'unknown'} if raw['score']=='unknown' else value(raw['score']))
  o=obs('saved-s05',fields,'partial',at=1791964800,until=1791964800,src='9b5a3c565f4aa9296ba98cdd7460138aa0642f38ded31470e9c8327cda12ee9c',pointer=row['source']['jsonPointer'],digest=row['source']['sha256'])
  recorded_views.append(view(ident(t,raw['id'],REC),[o],scope(REC,'synthetic-student-1','saved-subset')))
(P/'fixtures/synthetic.json').write_text(json.dumps(dict(objects=objects,links=links),indent=2)+'\n')
sv=view(student,[s]); bv=view(book,[b]); pv=view(progress,[p1,p2,p3])
def ok(objs): return {'status':'ok','authorization_revision':1,'result':{'kind':'objects','objects':objs}}
def decide(basis,decision): return {'status':'ok','authorization_revision':1,'result':{'kind':'decision','decision':decision,'basis':basis}}
cases=[]
def add(name,q,out,exit=0): cases.append(dict(id=name,input=q,expected=out,exit=exit))
add('ONT-01-leading-zero',{'op':'get','id':student},ok([sv]))
for k in ['1','001 ','a|b']:
 ob=next(o for o in objects if o['id']==ident('student',k)); add('ONT-01-exact-'+repr(k),{'op':'get','id':ob['id']},ok([view(ob['id'],ob['observations'])]))
ob=objects[-1]; add('ONT-01-delimiter-tuple',{'op':'get','id':ob['id']},ok([view(ob['id'],ob['observations'])]))
for name,i in [('other-account',objects[3]['id']),('other-student',ident('progress','secret')),('other-class',ident('progress','class-secret')),('absent',ident('student','Synthetic Student')),('no-name-guess',ident('student',' 001'))]:
 add('ONT-02-'+name,{'op':'get','id':i},{'status':'refused','code':'unavailable'},2)
# UTF-8 tuple sort: progress, student, textbook. Authorized exact student 1/001-space
# are valid identities but not linked in this graph (links were intentionally selected).
visible=[pv,view(objects[4]['id'],objects[4]['observations']),bv]
visible.sort(key=lambda x:(x['id']['object_type'],x['id']['namespace']['instance'],x['id']['namespace']['account'],x['id']['native_key']))
# objects[4] is student native '1'; learning also links student '001 '.
visible.append(view(objects[5]['id'],objects[5]['observations'])); visible.sort(key=lambda x:(x['id']['object_type'],x['id']['native_key']))
add('ONT-02-traverse',{'op':'traverse','root':student,'relation':'learning'},ok(visible))
add('ONT-04-history',{'op':'get','id':progress},ok([pv]))
add('ONT-04-decision',{'op':'decide','root':student,'relation':'learning'},decide(sorted([sv]+visible,key=lambda x:(x['id']['object_type'],x['id']['native_key'])),'review_required'))
add('useful-observed-books',{'op':'decide','root':student,'relation':'books'},decide([sv,bv],'evidence_available'))
add('empty-traversal',{'op':'traverse','root':student,'relation':'absent'},ok([]))
add('S05-recorded-student',{'op':'get','id':recorded_root},ok(recorded_views[:1]))
for t in ['section','attempt']:
 vs=sorted([x for x in recorded_views if x['id']['object_type']==t],key=lambda x:x['id']['native_key'])
 add('S05-'+t,{'op':'traverse','root':recorded_root,'relation':t},ok(vs))
 add('S05-'+t+'-decision',{'op':'decide','root':recorded_root,'relation':t},decide(sorted(recorded_views[:1]+vs,key=lambda x:(x['id']['object_type'],x['id']['native_key'])),'review_required'))
for key,val in [('principal','root'),('grants',['*']),('host_uid',0),('teacherLabel','박경찬')]:
 add('ONT-06-claimed-'+key,{'op':'get','id':student,key:val},{'status':'refused','code':'invalid_request'},2)
add('invalid-identity',{'op':'get','id':ident('student','')},{'status':'refused','code':'invalid_request'},2)
add('invalid-op',{'op':'action','root':student},{'status':'refused','code':'invalid_request'},2)
(P/'cases.json').write_text(json.dumps(cases,ensure_ascii=False,indent=2)+'\n')
print('Authored',len(cases),'literal cases before implementation')

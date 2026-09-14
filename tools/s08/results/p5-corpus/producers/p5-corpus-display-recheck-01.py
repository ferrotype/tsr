import json, shutil, sys
from pathlib import Path
sys.path.insert(0,'/Users/cristian/git/ts-rust/scripts')
import s08_p5_corpus as p5
p4=p5.p4
control=p5.ROOT/'target/s08/p5-corpus-rust-full-02'
output=p5.ROOT/'target/s08/p5-corpus-display-recheck-01'
output.mkdir()
prior=p5.read(control/'report.json')
selected=[i for i,r in enumerate(prior['rows']) if r.get('type_symbols',{}).get('state')=='failed']
wanted=set(selected);expected=[]
with (control/'native-observations.ndjson').open('rb') as stream:
 for index,line in enumerate(stream):
  if index in wanted:expected.append(p5.strict_json_loads(line))
requests=[p5.read(control/'cases'/f'{i:05d}'/'request.json') for i in selected]
assert [r['id'] for r in requests]==[r['id'] for r in expected]
record=p4.build(output/'build',example='p5_inventory',source_fn=p5.sources,optimize=True)
binary=output/'executable';shutil.copy2(record['binary'],binary)
metadata={'selection':'All type/symbol failures from the complete 5e93fce capture; supplemental comparison, no new full-corpus count',
 'control_capture_sha256':prior['capture_sha256'],'control_indexes':selected,
 'requests_sha256':p5.digest(p5.canonical(requests)+b'\n'),'expected_sha256':p5.digest(p5.canonical(expected)+b'\n'),
 'producer_sha256':p5.digest(Path(__file__).read_bytes()),'build':record,'timeout_seconds':60}
p5.write_new(output/'capture.json',metadata);p5.write_new(output/'requests.json',requests);p5.write_new(output/'expected.json',expected)
shutil.copy2(__file__,output/'producer.py');(output/'cases').mkdir()
rows=[]
for i,request in enumerate(requests):
 directory=output/'cases'/f'{i:05d}'
 row=p4.execute_case(binary,directory,request,60,validator=p5.validate_row)
 p4.complete_case(directory,request,metadata,row);rows.append(row)
 if (i+1)%100==0 or i+1==len(requests):print('Display recheck',i+1,'/',len(requests),flush=True)
_,observed,report=p4.replay(output,validator=p5.validate_row,summarizer=lambda reqs,rows:p5.summarize(reqs,rows,expected))
assert rows==observed
report['source_stable']=p5.sources()==record['sources']
p5.write_new(output/'report.json',report)
print(report['counts_by_tier'], 'source_stable',report['source_stable'])

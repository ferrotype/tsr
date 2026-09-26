#!/usr/bin/env python3
"""Capture the pinned property-elision branch without modifying upstream files."""
import hashlib,json,os,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[6]
FIX=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT/'scripts'))
from s04 import go_environment,verified_upstream
UP=verified_upstream()/'tsc'
OUT=ROOT/'target/phase2/c2-property-elision-native'
OUT.mkdir(parents=True,exist_ok=True)
env=go_environment()
env['GOCACHE']=str(OUT/'go-build')
sources=[FIX/'property_oracle_bridge.go',FIX/'property_oracle_test.go']
subprocess.run(['gofmt','-w',*[str(p) for p in sources]],env=env,check=True)
overlay=OUT/'overlay.json'
overlay.write_text(json.dumps({'Replace':{str(UP/'internal/checker'/('c2_'+p.name)):str(p) for p in sources}}))
cmd=['go','test','-mod=readonly','-trimpath','-overlay',str(overlay),'./internal/checker','-run','^TestC2PropertyElision$','-count=1','-timeout=2m']
env.update(C2_REQUESTS=str(FIX/'property.requests.json'),C2_OUTPUT=str(FIX/'property.observations.json'))
subprocess.run(cmd,cwd=UP,env=env,check=True)
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
receipt={'pin':json.loads((ROOT/'data/upstream.json').read_text())['pin'],'command':cmd,'request_sha256':sha(FIX/'property.requests.json'),'output_sha256':sha(FIX/'property.observations.json'),'observer_sources':{p.name:sha(p) for p in sources+[pathlib.Path(__file__)]},'scope':'Access-only property-elision branch observations at the pinned thresholds; actual source request tests separately establish naturally reached truncation.'}
(FIX/'property.provenance.json').write_text(json.dumps(receipt,separators=(',',':'))+'\n')

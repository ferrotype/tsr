#!/usr/bin/env python3
"""Produce direct contract observations in a separate pinned Go overlay."""
import hashlib,json,os,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[6]
FIX=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT/'scripts'))
from s04 import go_environment,verified_upstream
UP=verified_upstream()/'tsc'
OUT=ROOT/'target/phase2/c2-audit-gap-native'
OUT.mkdir(parents=True,exist_ok=True)
sha=lambda b:hashlib.sha256(b).hexdigest()
env=go_environment()
for p in [FIX/'oracle_bridge.go',FIX/'oracle_test.go']:
 subprocess.run(['gofmt','-w',str(p)],env=env,check=True)
mapping={str(UP/'internal/checker/c2_audit_gap_bridge.go'):str(FIX/'oracle_bridge.go'),str(UP/'internal/checker/c2_audit_gap_test.go'):str(FIX/'oracle_test.go')}
overlay=OUT/'overlay.json';overlay.write_text(json.dumps({'Replace':mapping}))
cmd=['go','test','-mod=readonly','-trimpath','-overlay',str(overlay),'./internal/checker','-run','^TestC2AuditGaps$','-count=1','-timeout=3m']
env.update(C2_REQUESTS=str(FIX/'requests.json'),C2_OUTPUT=str(FIX/'native.json'))
subprocess.run(cmd,cwd=UP,env=env,check=True)
receipt={'pin':json.loads((ROOT/'data/upstream.json').read_text())['pin'],'command':cmd,'request_sha256':sha((FIX/'requests.json').read_bytes()),'output_sha256':sha((FIX/'native.json').read_bytes()),'observer_sources':{p.name:sha(p.read_bytes()) for p in [FIX/'oracle_bridge.go',FIX/'oracle_test.go',FIX/'regenerate.py']},'replacements':{str(pathlib.Path(k).relative_to(UP)):sha(pathlib.Path(v).read_bytes()) for k,v in mapping.items()},'scope':'Actual production relation and suggestion calls on source; mapped-cycle state schedule explicitly models a completed intermediate resolution before pop using native production push/find/pop operations.'}
(FIX/'provenance.json').write_text(json.dumps(receipt,separators=(',',':'))+'\n')

#!/usr/bin/env python3
"""Record the checker's own diagnostics for the malformed fixture in a separate pinned Go overlay.

The request is built from ../malformed.ts so the source has one home; native.json and
provenance.json bind the pin, the request and the observer sources.
"""
import hashlib,json,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[6]
FIX=pathlib.Path(__file__).resolve().parent
sys.path.insert(0,str(ROOT/'scripts'))
from s04 import go_environment,verified_upstream
UP=verified_upstream()/'tsc'
OUT=ROOT/'target/phase2/c3-malformed-native'
OUT.mkdir(parents=True,exist_ok=True)
sha=lambda b:hashlib.sha256(b).hexdigest()
source=(FIX.parent/'malformed.ts').read_text()
(FIX/'requests.json').write_text(json.dumps([{"id":"malformed","root":"/main.ts","files":{"/main.ts":source}}],indent=1)+"\n")
env=go_environment()
subprocess.run(['gofmt','-w',str(FIX/'oracle_test.go')],env=env,check=True)
mapping={str(UP/'internal/checker/c3_malformed_test.go'):str(FIX/'oracle_test.go')}
overlay=OUT/'overlay.json';overlay.write_text(json.dumps({'Replace':mapping}))
cmd=['go','test','-mod=readonly','-trimpath','-overlay',str(overlay),'./internal/checker','-run','^TestC3Malformed$','-count=1','-timeout=3m']
env.update(C3_REQUESTS=str(FIX/'requests.json'),C3_OUTPUT=str(FIX/'native.json'))
subprocess.run(cmd,cwd=UP,env=env,check=True)
receipt={'pin':json.loads((ROOT/'data/upstream.json').read_text())['pin'],'command':cmd,'request_sha256':sha((FIX/'requests.json').read_bytes()),'output_sha256':sha((FIX/'native.json').read_bytes()),'observer_sources':{p.name:sha(p.read_bytes()) for p in [FIX/'oracle_test.go',FIX/'regenerate.py']},'source_sha256':sha((FIX.parent/'malformed.ts').read_bytes()),'replacements':{str(pathlib.Path(k).relative_to(UP)):sha(pathlib.Path(v).read_bytes()) for k,v in mapping.items()},'scope':'Production parser, binder and checker diagnostics of one malformed module: the C3.8 semantic set the command line withholds.'}
(FIX/'provenance.json').write_text(json.dumps(receipt,separators=(',',':'))+'\n')

"""Utility evidence must execute its frozen tests and preserve failed assertions."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
import s06_utilities as utility


def manifest():
    groups=[]
    for name,target,prefix in (('front','utilities_front',''),('middle','lib','utilities_middle::tests::'),
                               ('tail','utilities_tail',''),('accessors','node_accessors','')):
        groups.append({'name':name,'checker':'scripts/'+name+'.py','target':target,'prefix':prefix,'tests':[prefix+'fixture']})
    return {'version':1,'pin':'pin','groups':groups}


def output(name, passed=True):
    status='ok' if passed else 'FAILED'
    return (f'running 1 test\ntest {name} ... {status}\n\ntest result: {status}. '
            f'{int(passed)} passed; {int(not passed)} failed; 0 ignored; 0 measured; 20 filtered out; finished in 0.0s\n').encode()


class UtilityTests(unittest.TestCase):
    def test_exact_inventory_rejects_missing_extra_duplicate_or_changed_tests(self):
        group=manifest()['groups'][1]
        good=(group['tests'][0]+': test\nelsewhere::test: test\n').encode()
        utility.test_inventory(good,group)
        for raw in (b'', good+good,good+b'utilities_middle::tests::new: test\n',b'wrong: test\n'):
            with self.assertRaises(ValueError):utility.test_inventory(raw,group)

    def test_failed_assertion_is_false_but_missing_ignored_or_inconsistent_execution_is_invalid(self):
        name='fixture'
        for passed in (True,False):
            result=subprocess.CompletedProcess([],0 if passed else 101,output(name,passed),b'')
            self.assertEqual(utility.test_result(result,name),passed)
        for raw,code in ((b'',0),(output('other'),0),(output(name).replace(b'... ok',b'... ignored'),0),
                          (output(name),101),(output(name,False),0),(output(name,False),-6),
                          (output(name).replace(b'running 1 test',b'running 0 tests'),0)):
            with self.assertRaises(ValueError):utility.test_result(subprocess.CompletedProcess([],code,raw,b''),name)

    def test_manifest_rejects_missing_groups_duplicates_empty_tests_pin_and_paths(self):
        original=manifest()
        variants=[]
        changed=copy.deepcopy(original);changed['groups'].pop();variants.append(changed)
        changed=copy.deepcopy(original);changed['groups'][1]=changed['groups'][0];variants.append(changed)
        changed=copy.deepcopy(original);changed['groups'][1]['tests']=[];variants.append(changed)
        changed=copy.deepcopy(original);changed['groups'][1]['checker']='../outside.py';variants.append(changed)
        changed=copy.deepcopy(original);changed['groups'][1]['target']='utilities_front';changed['groups'][1]['prefix']='';variants.append(changed)
        changed=copy.deepcopy(original);changed['groups'][1]['name']=[];variants.append(changed)
        changed=copy.deepcopy(original);changed['pin']='wrong';variants.append(changed)
        changed=copy.deepcopy(original);changed['version']=True;variants.append(changed)
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);(root/'data/s06').mkdir(parents=True)
            (root/'data/upstream.json').write_text(json.dumps({'pin':'pin'}))
            path=root/'data/s06/utility-tests.json'
            path.write_text(json.dumps(original));utility.load_manifest(root)
            for changed in variants:
                path.write_text(json.dumps(changed))
                with self.assertRaises(ValueError):utility.load_manifest(root)

    def test_measured_failure_does_not_skip_other_families(self):
        data=manifest();invocations=[]
        def invoke(args, root, prefix, *, timeout=300):
            invocations.append(args)
            if args[0]=='cargo':
                rows=[{'reason':'compiler-artifact','executable':'/'+target,'target':{'name':'ts_ast' if target=='lib' else target,'kind':['lib'] if target=='lib' else ['test']},'profile':{'test':True}} for target in ('lib','utilities_front','utilities_tail','node_accessors')]
                raw=b'\n'.join(json.dumps(row).encode() for row in rows)
            elif '--list' in args:
                raw=''.join(name+': test\n' for group in data['groups'] if '/'+group['target']==args[0] for name in group['tests']).encode()
            elif '--exact' in args:
                passed=not args[1].startswith('utilities_middle::')
                return subprocess.CompletedProcess(args,0 if passed else 101,output(args[1],passed),b'assertion details')
            else:raw=b''
            return subprocess.CompletedProcess(args,0,raw,b'')
        with tempfile.TemporaryDirectory() as temporary,patch.object(utility,'load_manifest',return_value=(data,'digest')),patch.object(utility,'invoke',side_effect=invoke):
            result=utility.measure(Path(temporary),Path(temporary)/'reports')
            self.assertFalse(result['metric'])
            self.assertEqual((result['tests'],result['groups']),(4,4))
            self.assertEqual(len([args for args in invocations if '--exact' in args]),4)
            self.assertEqual(len([case for case in result['cases'] if not case['pass']]),1)

    def test_setup_or_fixture_check_failure_is_not_a_measured_rust_result(self):
        result=subprocess.CompletedProcess([],1,b'drift',b'')
        with patch.object(utility,'invoke',return_value=result):
            with self.assertRaisesRegex(RuntimeError,'setup/check failed'):
                utility.setup(['python3','check.py','--check'],Path('/root'),Path('/output'))

    def test_setup_forwards_explicit_budget_and_keeps_default_test_budget(self):
        result = subprocess.CompletedProcess([], 0, b'output', b'')
        with tempfile.TemporaryDirectory() as temporary, \
                patch.object(utility.subprocess, 'run', return_value=result) as run:
            root = Path(temporary)
            self.assertEqual(utility.setup(['build'], root, root/'build', timeout=1800), b'output')
            self.assertEqual(run.call_args.kwargs['timeout'], 1800)
            utility.setup(['list'], root, root/'list')
            self.assertEqual(run.call_args.kwargs['timeout'], 300)
            utility.invoke(['test'], root, root/'test')
            self.assertEqual(run.call_args.kwargs['timeout'], 300)

    def test_timeout_preserves_partial_output_and_cannot_become_success(self):
        for stdout, stderr in ((b'cargo artifact', b'Compiling ts_checker'), (None, None)):
            error = subprocess.TimeoutExpired(['cargo', 'test'], 1800, output=stdout, stderr=stderr)
            with self.subTest(stdout=stdout), tempfile.TemporaryDirectory() as temporary, \
                    patch.object(utility.subprocess, 'run', side_effect=error):
                root = Path(temporary)
                prefix = root/'build'
                with self.assertRaises(subprocess.TimeoutExpired):
                    utility.setup(['cargo', 'test'], root, prefix, timeout=1800)
                self.assertEqual(prefix.with_suffix('.stdout').read_bytes(), stdout or b'')
                self.assertEqual(prefix.with_suffix('.stderr').read_bytes(), stderr or b'')
                self.assertEqual(json.loads(prefix.with_suffix('.command.json').read_bytes()), {
                    'command': ['cargo', 'test'], 'returncode': None,
                    'timed_out': True, 'timeout_seconds': 1800,
                })


if __name__=='__main__':unittest.main()

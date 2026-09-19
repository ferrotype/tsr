"""P2 protocol never treats missing operations or partial inventories as parity."""
import copy
import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from s08_p2 import MODES, PHASES, ROOT, SPEC, canonical, compare, digest, specification, validate
from s04_common import strict_json_loads


class P2Protocol(unittest.TestCase):
    def fixture(self):
        spec=strict_json_loads((ROOT/SPEC).read_bytes())
        diagnostics={'state':'executed',**{phase:[] for phase in PHASES},'baseline':{'state':'no_content'}}
        programs=[]
        for request in spec['programs']:
            queries=[{'id':q['id'],'state':'executed','node':{},'symbol':None,
                      'type':{'flags':1,'object_flags':0,'display_hex':'','in_alias_display_hex':'','properties':[]}}
                     for q in request['queries']]
            programs.append({'id':request['id'],'state':'executed','queries':queries,'diagnostics':copy.deepcopy(diagnostics)})
        merges=[]
        for mode in MODES:
            single=mode in ('single-checker','repeated')
            owners=['single']*(1 if mode=='single-checker' else 6) if single else ['A','B','B','A','B']
            observations=[{'checker':owner,'merged_is_bound_base':False,'same_symbol_as_previous':True,'same_type_as_previous':True} for owner in owners]
            row={'mode':mode,'state':'executed','shared_bound_file_identity':True,'base_unchanged':True,
                 'base_before':{},'base_after':{},'observations':observations,'diagnostics':{'single':copy.deepcopy(diagnostics)}}
            if not single:
                row.update(different_merged_symbols=True,different_types=True)
                row['observations'][-1]['after_release_A']=True
            merges.append(row)
        return spec,{'version':1,'request_sha256':digest(canonical(spec)+b'\n'),'programs':programs,'merges':merges}

    def test_complete_inventory_can_be_validated(self):
        self.assertEqual(validate(*self.fixture()),[])

    def test_comparison_scope_comes_from_the_frozen_request(self):
        for scope in ('Historical P2 scope', 'Named P3 scope'):
            spec,observed=self.fixture()
            spec['scope']=scope
            request=canonical(spec)+b'\n'
            observed['request_sha256']=digest(request)
            native=canonical(observed)+b'\n'
            observed['rust_ownership']={'scope':'Rust-only lifetime checks','programs':[
                {'id':p['id'],'state':'executed','program_survives_retained_result':True,
                 'retained_display_unchanged':True,'program_released_after_result_drop':True}
                for p in spec['programs']]}
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as tmp:
                directory=Path(tmp)
                (directory/'requests.json').write_bytes(request)
                (directory/'observations.json').write_bytes(native)
                (directory/'report.json').write_bytes(canonical({'sources':{},
                    'request_sha256':digest(request),'observation_sha256':digest(native)}))
                (directory/'actual.json').write_bytes(canonical(observed)+b'\n')
                with contextlib.redirect_stdout(io.StringIO()):
                    result=compare(directory,directory/'actual.json',directory/'verified.json')
                self.assertTrue(result['matched'])
                self.assertEqual(result['scope'],scope)

    def test_options_and_execution_modes_are_explicit(self):
        for key,value in (('noLib',False),('strict',False),('module','commonjs'),('strict',1)):
            spec,_=self.fixture();spec['options'][key]=value
            with self.subTest(key=key), self.assertRaises(ValueError):specification(spec)
        spec,_=self.fixture();spec['merge']['modes']=MODES[:-1]
        with self.assertRaises(ValueError):specification(spec)

    def test_library_policy_and_summary_do_not_weaken_full_queries(self):
        spec,observed=self.fixture()
        spec['options']={'target':'ESNext','module':'ESNext','strict':True,'noLib':False,'skipLibCheck':True}
        specification(spec)
        first=spec['programs'][0]['queries'][0]
        first['operation']='declared_type_summary'; first['target']='name'
        observed['request_sha256']=digest(canonical(spec)+b'\n')
        del observed['programs'][0]['queries'][0]['type']['properties']
        self.assertEqual(validate(spec,observed),[])
        first['operation']='declared_type'
        observed['request_sha256']=digest(canonical(spec)+b'\n')
        with self.assertRaises(ValueError):validate(spec,observed)

    def test_missing_queries_and_reordered_programs_fail(self):
        spec,observed=self.fixture();observed['programs'][0]['queries'].pop()
        with self.assertRaises(ValueError):validate(spec,observed)
        spec,observed=self.fixture();observed['programs'].reverse()
        with self.assertRaises(ValueError):validate(spec,observed)

    def test_query_and_diagnostics_unsupported_are_named_and_separate(self):
        spec,observed=self.fixture();program=observed['programs'][0];query=program['queries'][0]
        program['queries'][0]={'id':query['id'],'state':'unsupported','operation':'get_declared_type_of_symbol','reason':'type alias resolution not implemented'}
        program['diagnostics']={'state':'unsupported','operation':'diagnostics','reason':'source file checking not implemented'}
        with self.assertRaises(ValueError):validate(spec,observed)
        missing=validate(spec,observed,allow_unsupported=True)
        self.assertEqual([m['operation'] for m in missing],['get_declared_type_of_symbol','diagnostics'])
        del program['queries'][0]['reason']
        with self.assertRaises(ValueError):validate(spec,observed,allow_unsupported=True)

    def test_merge_sequence_and_source_immutability_are_required(self):
        for change in ('identity','mutation','release','sequence','repeat'):
            spec,observed=self.fixture();merge=observed['merges'][-1]
            if change=='identity':merge['shared_bound_file_identity']=False
            if change=='mutation':merge['base_after']={'changed':True}
            if change=='release':del merge['observations'][-1]['after_release_A']
            if change=='sequence':merge['observations'].pop()
            if change=='repeat':merge['observations'][-1]['same_symbol_as_previous']=False
            with self.subTest(change=change), self.assertRaises(ValueError):validate(spec,observed)

    def test_semantic_errors_cannot_be_replaced_by_empty_baselines(self):
        spec,observed=self.fixture();diagnostics=observed['programs'][0]['diagnostics']
        payload={'file':None,'pos':-1,'end':-1,'code':2318,'category':1,'key_hex':'','text_hex':'','source_hex':'',
                 'args':['Array'],'chain':[],'related':[],'unnecessary':False,'deprecated':False,'skipped_on_no_emit':False}
        diagnostics['global']=[payload];diagnostics['combined']=[payload]
        with self.assertRaises(ValueError):validate(spec,observed)
        diagnostics['baseline']={'state':'content','text_hex':'6572726f72'}
        self.assertEqual(validate(spec,observed),[])
        del diagnostics['global'][0]['chain']
        with self.assertRaises(ValueError):validate(spec,observed)

    def test_failed_is_not_an_unsupported_or_successful_operation(self):
        spec,observed=self.fixture();observed['programs'][0]['state']='failed'
        with self.assertRaises(ValueError):validate(spec,observed,allow_unsupported=True)

    def test_baseline_unsupported_preserves_completed_diagnostics(self):
        spec,observed=self.fixture();diagnostics=observed['programs'][0]['diagnostics']
        diagnostics['baseline']={'state':'unsupported','operation':'error_baseline','reason':'related information decoration'}
        with self.assertRaises(ValueError):validate(spec,observed)
        missing=validate(spec,observed,allow_unsupported=True)
        self.assertEqual([m['operation'] for m in missing],['error_baseline'])
        diagnostics['semantic']={'state':'unsupported','operation':'semantic_diagnostics','reason':'value checking'}
        with self.assertRaises(ValueError):validate(spec,observed,allow_unsupported=True)
        diagnostics['combined']={'state':'unsupported','operation':'combined_diagnostics','reason':'incomplete semantic phase'}
        missing=validate(spec,observed,allow_unsupported=True)
        self.assertEqual([m['operation'] for m in missing],['semantic_diagnostics','combined_diagnostics','error_baseline'])
        self.assertEqual(diagnostics['global'],[])
        diagnostics['baseline']={'state':'no_content'}
        with self.assertRaises(ValueError):validate(spec,observed,allow_unsupported=True)

    def test_rust_lifetime_checks_are_separate_and_cannot_be_ignored(self):
        spec,observed=self.fixture()
        with self.assertRaises(ValueError):validate(spec,observed,require_rust_ownership=True)
        observed['rust_ownership']={'scope':'Rust-only lifetime checks','programs':[
            {'id':p['id'],'state':'executed','program_survives_retained_result':True,
             'retained_display_unchanged':True,'program_released_after_result_drop':True}
            for p in spec['programs']]}
        self.assertEqual(validate(spec,observed,require_rust_ownership=True),[])
        observed['rust_ownership']['programs'][0]['program_released_after_result_drop']=False
        with self.assertRaises(ValueError):validate(spec,observed)

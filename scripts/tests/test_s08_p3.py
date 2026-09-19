"""Protocol adversaries for scoped P3 replays; no fabricated acceptance credit."""
import copy
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from s08_contracts import MODES
from s08_oracle import canonical,digest
from s08_p3_relations import compare as relations
from s08_p3_comparators import compare as comparators


class P3Relations(unittest.TestCase):
    def fixture(self):
        action={'mode':'assignable','report_errors':True,'source':'A','target':'B'}
        state={'caches':{m:{'entries':0,'result_flags':[]} for m in MODES},'instantiations':0,'signatures_created':1,'types_created':1}
        observed={'action':action,'before':state,'after':state,'result':False,'ternary_calls':[0],'diagnostics':[]}
        request=canonical([{'id':'recursive','actions':[action]},{'id':'body','actions':[action]}])
        native={'request_sha256':digest(request),'rows':[{'id':name,'groups':[{'actions':[observed],'before_lookup':state}]} for name in ('recursive','body')]}
        native=canonical(native)
        pending={'checkpoint':'P4','operation':'body inference','reason':'not implemented'}
        policy={'version':1,'request_sha256':digest(request),'native_sha256':digest(native),'pending_cases':{'body':pending},'pending_protocol':['post_action_display','post_action_union_ordering','final_state','source_and_global_diagnostics']}
        actual={'version':1,'request_sha256':digest(request),'rows':[
            {'id':'recursive','groups':[{'mode':'assignable','observations':{'before_lookup':state,'actions':[observed],'state':'observed','display_state':'pending'}}]},
            {'id':'body','groups':[{'mode':'assignable','observations':{'state':'pending','reason':'not implemented'}}]}]}
        return request,native,policy,copy.deepcopy(actual)

    def test_only_executed_actions_receive_scoped_credit(self):
        result=relations(*self.fixture())
        self.assertEqual((result['executed_actions'],result['pending_actions']),(1,1))
        self.assertTrue(result['scoped_actions_match'])
        self.assertFalse(result['full_p0_contract_match'])

    def test_bad_provenance_inventory_counters_result_and_pending_reason_fail(self):
        for change in ('hash','row','mode','action','counter','boolean','diagnostic','unexpected_pending','pending_reason'):
            request,native,policy,actual=self.fixture()
            with self.subTest(change=change):
                observation=actual['rows'][0]['groups'][0]['observations']
                if change=='hash':actual['request_sha256']='0'*64
                elif change=='row':actual['rows'].pop()
                elif change=='mode':actual['rows'][0]['groups'].clear()
                elif change=='action':observation['actions'].clear()
                elif change=='counter':observation['actions'][0]['after']['types_created']=2
                elif change=='boolean':observation['actions'][0]['result']=0
                elif change=='diagnostic':observation['actions'][0]['diagnostics']=[{}]
                elif change=='unexpected_pending':actual['rows'][0]['groups'][0]['observations']={'state':'pending','reason':'not implemented'}
                else:actual['rows'][1]['groups'][0]['observations']['reason']='different error'
                with self.assertRaises(ValueError):relations(request,native,policy,actual)


class P3Comparators(unittest.TestCase):
    def fixture(self):
        request=canonical([{'id':'union'}]); matrix=[[0,-3],[3,0]];order={'input':[1,0],'sorted':[0,1]}
        native=canonical({'request_sha256':digest(request),'rows':[{'id':'union','groups':[{'union_ordering':[{'type':'A','pairwise':matrix},{'type':'A',**order}]}]}],
            'supplemental':{'residuals':{'mapper':matrix,'foreign-checker':{'state':'panic','message':'Cannot compare types from different checkers'}}}})
        header={'request_sha256':digest(request),'native_sha256':digest(native)}
        inputs={'version':1,**header,'cases':{'union':[{'type':'A','inputs':[[1,0]]}]}}
        actual={**header,'foreign_checker':{'state':'rejected','error':'Arena(WrongOwner)'},'residuals':{'mapper':[[0,-1],[1,0]]},
            'rows':[{'id':'union','ordering':[{'state':'executed','type':'A','pairwise':[[0,-1],[1,0]],'permutations':[order]}]}]}
        return request,native,inputs,actual

    def test_direct_comparisons_use_signs_and_exact_permutations(self):
        result=comparators(*self.fixture())
        self.assertTrue(result['matched']); self.assertEqual(result['permutations'],1)

    def test_omitted_changed_or_wrong_owner_observations_fail(self):
        for change in ('hash','residual','foreign','matrix','permutation','boolean'):
            request,native,inputs,actual=self.fixture()
            row=actual['rows'][0]['ordering'][0]
            if change=='hash':inputs['native_sha256']='0'*64
            elif change=='residual':actual['residuals'].clear()
            elif change=='foreign':actual['foreign_checker']={'state':'returned','value':0}
            elif change=='matrix':row['pairwise'][0][1]=1
            elif change=='permutation':row['permutations'].clear()
            else:row['pairwise'][0][0]=False
            with self.subTest(change=change), self.assertRaises(ValueError):comparators(request,native,inputs,actual)


if __name__=='__main__':unittest.main()

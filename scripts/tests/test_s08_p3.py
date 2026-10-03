"""Protocol adversaries for the P3 comparator replay the E2 obligations use."""
import copy
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from s08_oracle import canonical,digest
from s08_p3_comparators import compare as comparators


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

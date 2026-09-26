"""A source witness must reach an observed natural fallback, not fabricated ids."""
import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import phase2_order_trace as trace


class CreationTraceContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.frozen = trace.verify_frozen()

    def pair(self, index=1):
        return self.frozen["manifest"]["witnesses"][index], copy.deepcopy(self.frozen["rows"][index]["on"])

    def test_all_four_native_traces_replay(self):
        for witness,row in zip(self.frozen["manifest"]["witnesses"],self.frozen["rows"],strict=True):
            self.assertEqual(trace.canonical(trace.validate_trace(witness,row["on"])),trace.canonical(row["summary"]))
            self.assertEqual(row["on"]["ordinary"],row["off"]["ordinary"])

    def test_a_label_cannot_assign_a_missing_origin(self):
        witness,row=self.pair()
        event=next(e for e in row["trace"] if e["event"]=="id_assignment")
        event["token"]=None
        with self.assertRaisesRegex(ValueError,"unobserved"):
            trace.validate_trace(witness,row)

    def test_missing_birth_and_double_assignment_fail(self):
        for mutation in ("birth","assignment"):
            witness,row=self.pair()
            assignment=next(e for e in row["trace"] if e["event"]=="id_assignment")
            if mutation=="birth":
                row["trace"]=[e for e in row["trace"] if not(e["event"]=="birth" and e["kind"]=="symbol" and e["token"]==assignment["token"])]
            else:
                row["trace"].insert(row["trace"].index(assignment)+1,copy.deepcopy(assignment))
            with self.assertRaises(ValueError): trace.validate_trace(witness,row)

    def test_unreached_branch_cannot_claim_witness_coverage(self):
        witness,row=self.pair()
        row["trace"]=[e for e in row["trace"] if e["event"]!="fallback"]
        with self.assertRaisesRegex(ValueError,"actual comparator fallback"):
            trace.validate_trace(witness,row)

    def test_sign_must_follow_the_actual_assigned_ids(self):
        witness,row=self.pair()
        event=next(e for e in row["trace"] if e["event"]=="fallback")
        event["sign"]=-event["sign"]
        with self.assertRaisesRegex(ValueError,"sign"):
            trace.validate_trace(witness,row)

    def test_same_declaration_branch_checks_both_declarations(self):
        witness,row=self.pair(2)
        event=next(e for e in row["trace"] if e["event"]=="fallback")
        event["right"]["declarations"][0]["pos"]+=1
        with self.assertRaisesRegex(ValueError,"reported tie"):
            trace.validate_trace(witness,row)

    def test_sorts_must_have_input_and_retain_operands(self):
        witness,row=self.pair()
        event=next(e for e in row["trace"] if e["event"]=="sort_end")
        event["values"].clear()
        with self.assertRaisesRegex(ValueError,"retain"):
            trace.validate_trace(witness,row)

    def test_reverse_mapped_claim_requires_real_reverse_mapped_operands(self):
        witness,row=self.pair(3)
        for value in row["types"]: value["object_flags"] &= ~1024
        with self.assertRaisesRegex(ValueError,"not reverse mapped"):
            trace.validate_trace(witness,row)


if __name__=="__main__": unittest.main()

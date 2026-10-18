"""Trusted acceptance tests: no candidate imports."""
import unittest
from candidate_client import call

def b(identity,start,end,units=1):
    return {"id":identity,"start":start,"end":end,"units":units}
def r(identity,earliest,latest,duration,units=1):
    return {"id":identity,"earliest":earliest,"latest":latest,"duration":duration,"units":units}
def span(start,end,used):
    return {"start":start,"end":end,"used":used}

class Acceptance(unittest.TestCase):
    def plan(self, capacity, bookings, requests):
        args=[capacity,bookings,requests]
        reply=call("planner","plan",args)
        self.assertEqual(reply["args_after"],args)
        return reply["result"]
    def test_empty(self):
        self.assertEqual(self.plan(2,[],[]),{"accepted":[],"rejected":[],"profile":[],"peak":0})
    def test_half_open_boundary(self):
        result=self.plan(1,[b("old",0,5)],[r("new",5,8,3)])
        self.assertEqual(result,{"accepted":[b("new",5,8)],"rejected":[],"profile":[span(0,8,1)],"peak":1})
    def test_wait_for_release(self):
        self.assertEqual(self.plan(1,[b("old",0,5)],[r("new",1,10,3)])["accepted"],[b("new",5,8)])
    def test_simultaneous_capacity(self):
        result=self.plan(3,[b("a",0,6,2)],[r("x",1,8,2,2),r("y",1,4,2)])
        self.assertEqual(result["accepted"],[b("x",6,8,2),b("y",1,3)])
        self.assertEqual(result["peak"],3)
    def test_reject_and_continue(self):
        result=self.plan(1,[b("a",0,10)],[r("x",1,4,2),r("y",10,12,2)])
        self.assertEqual(result["rejected"],["x"])
        self.assertEqual(result["accepted"],[b("y",10,12)])
    def test_order_is_not_sort_order(self):
        result=self.plan(1,[],[r("z",0,4,3),r("a",0,4,2)])
        self.assertEqual(result["accepted"],[b("z",0,3)])
        self.assertEqual(result["rejected"],["a"])
    def test_profile_overlap_and_gaps(self):
        result=self.plan(5,[b("a",0,4,2),b("b",2,6),b("c",9,10)],[])
        self.assertEqual(result["profile"],[span(0,2,2),span(2,4,3),span(4,6,1),span(9,10,1)])
        self.assertEqual(result["peak"],3)
    def test_profile_coalesces(self):
        self.assertEqual(self.plan(2,[b("a",0,2),b("b",2,4)],[])["profile"],[span(0,4,1)])
    def test_negative_and_large_times(self):
        self.assertEqual(self.plan(1,[b("a",-10,10**12)],[r("x",-5,10**12+3,3)])["accepted"],[b("x",10**12,10**12+3)])
    def test_initial_overcapacity(self):
        self.assertEqual(self.plan(2,[b("a",0,3,2),b("b",2,4)],[]),{"error":"invalid"})
    def test_invalid_values(self):
        for capacity,bookings,requests in [(True,[],[]),(0,[],[]),(1,[b("a",2,2)],[]),(1,[],[r("a",0,1,2)]),(1,[],[r("a",0,1,1,True)]),(1,None,[]),(1,[],[{}])]:
            with self.subTest(args=(capacity,bookings,requests)):
                self.assertEqual(self.plan(capacity,bookings,requests),{"error":"invalid"})
    def test_duplicate_ids_and_validate_all(self):
        self.assertEqual(self.plan(1,[b("a",0,1)],[r("a",2,3,1)]),{"error":"invalid"})
        self.assertEqual(self.plan(1,[],[r("a",0,1,1),r("b",0,2,0)]),{"error":"invalid"})
    def test_full_duration_must_fit(self):
        self.assertEqual(self.plan(1,[b("a",3,5)],[r("x",0,8,4)])["rejected"],["x"])
    def test_small_exhaustive_earliest(self):
        # Independent integer-time oracle, intentionally only on tiny horizons.
        for start,end in [(0,2),(1,3),(2,5),(4,6)]:
            for duration in [1,2,3]:
                expected=next((t for t in range(8-duration+1) if t+duration<=start or t>=end),None)
                result=self.plan(1,[b("old",start,end)],[r("new",0,8,duration)])
                self.assertEqual(result["accepted"],[] if expected is None else [b("new",expected,expected+duration)])

    def test_combined_load_release(self):
        result=self.plan(3,[b("a",0,4),b("b",0,7)],[r("x",0,8,3,2)])
        self.assertEqual(result["accepted"],[b("x",4,7,2)])
        self.assertEqual(result["profile"],[span(0,4,2),span(4,7,3)])
    def test_fits_before_busy_interval(self):
        result=self.plan(2,[b("busy",3,6,2)],[r("x",0,9,3,2),r("y",0,9,3,2)])
        self.assertEqual(result["accepted"],[b("x",0,3,2),b("y",6,9,2)])
        self.assertEqual(result["profile"],[span(0,9,2)])

if __name__ == "__main__":
    unittest.main()

"""Trusted checks: candidate modules execute exclusively via JSON RPC."""
import unittest
from candidate_client import call

def route(path, handler="h", method="GET"):
    return {"path":path,"handler":handler,"method":method}

class Acceptance(unittest.TestCase):
    def run_route(self, routes, target="/", method="GET"):
        args = [routes, {"method":method,"target":target}]
        reply = call("router", "dispatch", args)
        self.assertEqual(reply["args_after"], args)
        return reply["result"]

    def ok(self, routes, target, handler="h", params=None, query=None):
        self.assertEqual(self.run_route(routes,target), {"status":200,"handler":handler,"params":params or {},"query":query or {}})

    def test_literal_precedence(self):
        self.ok([route("/:id","param"),route("/new","literal")],"/new","literal")
    def test_parameter_capture(self):
        self.ok([route("/u/:id/posts/:slug")],"/u/12/posts/hi",params={"id":"12","slug":"hi"})
    def test_wildcard(self):
        self.ok([route("/files/*rest")],"/files/a/b",params={"rest":"a/b"})
    def test_empty_wildcard_and_exact(self):
        self.ok([route("/a/*rest","wild"),route("/a","exact")],"/a","exact")
        self.ok([route("/a/*rest")],"/a",params={"rest":""})
    def test_lexicographic_ranking(self):
        self.ok([route("/a/*r","wild"),route("/a/:x/c","specific")],"/a/b/c","specific",{"x":"b"})
    def test_registration_ties(self):
        self.ok([route("/:x","first"),route("/:y","second")],"/z","first",{"x":"z"})
    def test_methods_and_allowed(self):
        routes=[route("/a",method="post"),route("/a",method="GET"),route("/a",method="POST")]
        self.assertEqual(self.run_route(routes,"/a","HEAD"),{"status":405,"allowed":["GET","POST"]})
        self.assertEqual(self.run_route(routes,"/a","pOsT")["status"],200)
    def test_not_found(self):
        self.assertEqual(self.run_route([route("/a/:x")],"/a"),{"status":404})
        self.assertEqual(self.run_route([],"/"),{"status":404})
    def test_query_repeats(self):
        self.ok([route("/")],"/?a=1&a=2&flag&x=",query={"a":["1","2"],"flag":[""],"x":[""]})
    def test_query_encoding(self):
        self.ok([route("/:x")],"/%2F?q=a+b&q=%E2%98%83&=v&&bad=%ZZ",params={"x":"%2F"},query={"q":["a b","☃"],"":["v"],"bad":["%ZZ"]})
    def test_root_and_trailing_slash(self):
        self.ok([route("/a/")],"/a")
        self.ok([route("/")],"/")
    def test_invalid_patterns(self):
        for path in ["a","/a//b","/:x/:x","/*r/end","/:9bad","/a:b","/a?x","//"]:
            with self.subTest(path=path):
                self.assertEqual(self.run_route([route("/"),route(path)]),{"error":"invalid"})
    def test_invalid_shapes(self):
        for routes,request in [(None,{}),([{}],{}),([route("/",method="G3T")],{"method":"GET","target":"/"}),([],{"method":"GET","target":"/a//b"}),([],{"method":"GET","target":"/#x"}),([],{"method":1,"target":"/"})]:
            with self.subTest(routes=routes,request=request):
                self.assertEqual(call("router","dispatch",[routes,request])["result"],{"error":"invalid"})
    def test_literal_beats_capture_property(self):
        for name in ["alpha","0","snow-☃","new"]:
            self.ok([route("/*all","wild"),route("/:id","param"),route("/"+name,"literal")],"/"+name,"literal")

    def test_method_filter_before_specificity(self):
        self.ok([route("/:id","generic"),route("/special","post","POST")],"/special","generic",{"id":"special"})
    def test_query_replacement_and_equals(self):
        self.ok([route("/")],"/?utf=%FF&x=a=b&question=?",query={"utf":["�"],"x":["a=b"],"question":["?"]})
    def test_complete_validation_before_dispatch(self):
        self.assertEqual(self.run_route([route("/"),route("/unused",handler=4)]),{"error":"invalid"})
        self.assertEqual(call("router","dispatch",[[],None])["result"],{"error":"invalid"})

if __name__ == "__main__":
    unittest.main()

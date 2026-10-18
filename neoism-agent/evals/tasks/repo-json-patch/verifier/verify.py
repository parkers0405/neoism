"""Trusted JSON-RPC acceptance checks, no candidate imports."""
import unittest
from candidate_client import call

def op(kind,path,**fields):
    return dict(op=kind,path=path,**fields)

class Acceptance(unittest.TestCase):
    def patch(self, document, operations):
        args=[document,operations]
        reply=call("patch","apply_patch",args)
        self.assertEqual(reply["args_after"],args)
        return reply["result"]
    def ok(self, document, operations, expected):
        self.assertEqual(self.patch(document,operations),{"ok":True,"document":expected})
    def bad(self, document, operations, index, error):
        self.assertEqual(self.patch(document,operations),{"ok":False,"document":document,"index":index,"error":error})
    def test_object_workflow(self):
        self.ok({"a":1},[op("add","/b",value=2),op("replace","/a",value=3),op("remove","/b")],{"a":3})
    def test_array_insert_append(self):
        self.ok([1,3],[op("add","/1",value=2),op("add","/-",value=4),op("add","/4",value=5)],[1,2,3,4,5])
    def test_pointer_escapes(self):
        self.ok({"a/b":{"~key":1},"~1":2},[op("replace","/a~1b/~0key",value=9),op("test","/~01",value=2)],{"a/b":{"~key":9},"~1":2})
    def test_empty_key_and_root(self):
        self.ok({"":1},[op("replace","/",value=2)],{"":2})
        self.ok([1],[op("replace","",value={"x":1})],{"x":1})
        self.ok({"a":1},[op("remove","")],None)
        self.ok(None,[op("add","",value=[2])],[2])
    def test_copy_is_deep(self):
        self.ok({"a":{"x":[1]}},[op("copy","/b",**{"from":"/a"}),op("add","/b/x/-",value=2)],{"a":{"x":[1]},"b":{"x":[1,2]}})
    def test_move_post_removal_index(self):
        self.ok(["a","b","c","d"],[op("move","/2",**{"from":"/0"})],["b","c","a","d"])
    def test_move_same_and_descendant(self):
        self.ok({"a/b":1},[op("move","/a~1b",**{"from":"/a~1b"})],{"a/b":1})
        self.bad({"a":{}},[op("move","/a/b",**{"from":"/a"})],0,"path")
        self.bad({},[op("move","/x",**{"from":"/x"})],0,"path")
    def test_transaction_rollback(self):
        self.bad({"a":[1]},[op("add","/a/-",value=2),op("remove","/missing")],1,"path")
        self.bad([1,2],[op("move","/9",**{"from":"/0"})],0,"path")
    def test_test_json_equality(self):
        self.ok({"a":[1,False]},[op("test","",value={"a":[1.0,False]})],{"a":[1,False]})
        self.bad({"a":[True]},[op("test","/a",value=[1])],0,"test")
        self.bad(None,[op("test","",value=False)],0,"test")
    def test_missing_parents(self):
        for kind in ["add","replace","remove","test"]:
            with self.subTest(kind=kind):
                self.bad({},[op(kind,"/a/b",value=1)],0,"path")
        self.bad({"x":2},[op("add","/x/a",value=1)],0,"path")
    def test_array_indices(self):
        for token in ["01","-1","+1","1.0","-","2"]:
            with self.subTest(token=token):
                self.bad([0,1],[op("remove","/"+token)],0,"path")
        self.ok({"01":1},[op("replace","/01",value=2)],{"01":2})
    def test_invalid_operations_and_pointers(self):
        for operation in [None,{},op("unknown",""),op("add","/x"),op("remove","x"),op("remove","/~2"),op("copy","/x",**{"from":"/~"}),op("move","/~2",**{"from":"/missing"})]:
            with self.subTest(operation=operation):
                self.bad({},[operation],0,"invalid")
        self.bad({},None,None,"invalid")
    def test_first_failure_and_empty(self):
        self.bad({},[op("test","/x",value=1),None],0,"path")
        for document in [None,False,3,"text",[],{}]:
            self.ok(document,[],document)
    def test_insert_remove_property(self):
        for length in range(5):
            original=list(range(length))
            for index in range(length+1):
                with self.subTest(length=length,index=index):
                    self.ok(original,[op("add","/"+str(index),value={"x":1}),op("remove","/"+str(index))],original)

    def test_result_and_input_independence(self):
        args=[{"keep":[1]},[op("add","/new",value={"items":[]})]]
        probes=[
            {"target":"result","path":["document","keep"],"op":"append","value":2},
            {"target":"result","path":["document","new","items"],"op":"append","value":3},
            {"target":"args","path":[0,"keep"],"op":"append","value":4},
            {"target":"args","path":[1,0,"value","items"],"op":"append","value":5},
        ]
        reply=call("patch","apply_patch",args,mutations=probes)
        original={"ok":True,"document":{"keep":[1],"new":{"items":[]}}}
        self.assertEqual(reply["result"],original)
        self.assertEqual(reply["args_after"],args)
        self.assertEqual(reply["snapshots"],[
            {"result":{"ok":True,"document":{"keep":[1,2],"new":{"items":[]}}},"args":args},
            {"result":{"ok":True,"document":{"keep":[1,2],"new":{"items":[3]}}},"args":args},
            {"result":{"ok":True,"document":{"keep":[1,2],"new":{"items":[3]}}},"args":[{"keep":[1,4]},args[1]]},
            {"result":{"ok":True,"document":{"keep":[1,2],"new":{"items":[3]}}},"args":[{"keep":[1,4]},[op("add","/new",value={"items":[5]})]]},
        ])

    def test_copy_root_and_move_to_root(self):
        self.ok({"a":1},[op("copy","/snapshot",**{"from":""})],{"a":1,"snapshot":{"a":1}})
        self.ok({"a":[1,2],"b":3},[op("move","",**{"from":"/a"})],[1,2])
        self.ok([1,2,3],[op("move","/-",**{"from":"/0"})],[2,3,1])
    def test_failed_test_rolls_back(self):
        self.bad({"x":1},[op("replace","/x",value=2),op("test","/x",value=1)],1,"test")
        self.bad({"x":1},[op("copy","/new",**{"from":"/missing"})],0,"path")

if __name__ == "__main__":
    unittest.main()

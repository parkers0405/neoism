import unittest
from router import dispatch

class PublicTests(unittest.TestCase):
    def test_literal(self):
        self.assertEqual(dispatch([{"method":"GET","path":"/","handler":"home"}], {"method":"GET","target":"/"})["handler"], "home")

    def test_missing(self):
        self.assertEqual(dispatch([], {"method":"GET","target":"/missing"}), {"status":404})

if __name__ == "__main__":
    unittest.main()

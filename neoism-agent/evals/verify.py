"""Trusted verifier entrypoint; distinguishes failed assertions from harness errors."""

import json
import os
import sys
import unittest


def main():
    if os.geteuid() != 0:
        raise RuntimeError("Verifier supervisor must run as root; candidate subprocesses drop privileges")
    source = sys.stdin.read(1024 * 1024 + 1)
    if not source or len(source) > 1024 * 1024:
        raise RuntimeError("Missing or oversized hidden verifier source")
    namespace = {"__name__": "eval_task_verifier", "__file__": "<hidden-verifier>"}
    exec(compile(source, "<hidden-verifier>", "exec"), namespace)
    cases = [value for value in namespace.values()
             if isinstance(value, type) and issubclass(value, unittest.TestCase)
             and value is not unittest.TestCase]
    suite = unittest.TestSuite(unittest.defaultTestLoader.loadTestsFromTestCase(case) for case in cases)
    if suite.countTestCases() == 0:
        raise RuntimeError("Verifier collected no tests")
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    print(json.dumps({"type": "verification_result", "tests_run": result.testsRun,
                      "failures": len(result.failures), "errors": len(result.errors),
                      "skipped": len(result.skipped)}), flush=True)
    return 0 if result.wasSuccessful() else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(json.dumps({"type": "verification_error", "error": str(error)}), flush=True)
        sys.exit(2)

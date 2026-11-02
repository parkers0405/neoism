#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('dispatcher', Path(__file__).with_name('dispatch-nightly-release.py'))
dispatcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dispatcher)


class NightlyDispatchTests(unittest.TestCase):
    def test_initial_and_following_nightlies_share_scope_and_pin_source(self):
        sha = 'a' * 40
        for previous in [subprocess.CompletedProcess([], 1, '', 'HTTP 404'),
                         subprocess.CompletedProcess([], 0, '{"object":{"sha":"' + 'b' * 40 + '"}}', '')]:
            with patch.object(dispatcher.subprocess, 'run', side_effect=[previous, None, None]) as run:
                dispatcher.dispatch('owner/repo', 'v0.7.143-nightly.20261102.4', sha)
                update = run.call_args_list[1].args[0]
                if 'PATCH' in update:
                    self.assertIn('force=false', update)
                else:
                    self.assertIn('ref=refs/heads/nightly-builds', update)
                command = run.call_args_list[-1].args[0]
                self.assertIn(f'source_ref={sha}', command)
                self.assertEqual(command[command.index('--ref') + 1], 'nightly-builds')

    def test_ref_lookup_errors_do_not_create_or_dispatch(self):
        with patch.object(dispatcher.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1, '', 'HTTP 403')) as run:
            with self.assertRaises(RuntimeError):
                dispatcher.dispatch('owner/repo', 'v0.7.143-nightly.20261102.4', 'a' * 40)
            self.assertEqual(run.call_count, 1)

    def test_stable_and_unpinned_sources_are_rejected(self):
        for tag, sha in [('v0.7.143', 'a' * 40), ('v0.7.143-nightly.1', 'main')]:
            with self.assertRaises(ValueError):
                dispatcher.dispatch('owner/repo', tag, sha)


if __name__ == '__main__':
    unittest.main()
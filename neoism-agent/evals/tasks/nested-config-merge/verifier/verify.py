"""Trusted acceptance checks; candidate code runs only through JSON RPC."""
import copy
import unittest

from candidate_client import call


def merge_config(base, override):
    response = call('config_merge', 'merge_config', [base, override])
    if response['args_after'] != [base, override]:
        raise AssertionError('Candidate changed its inputs')
    return response['result']


class MergeAcceptance(unittest.TestCase):
    def check_probes(self, base, override, expected, probes):
        response = call('config_merge', 'merge_config', [base, override], mutations=probes)
        self.assertEqual(response['result'], expected)
        self.assertEqual(response['args_after'], [base, override])
        # Independent expected objects: replaying probes must not affect the
        # opposite side. Compare every snapshot, not just the final state.
        state = {'result': copy.deepcopy(expected), 'args': copy.deepcopy([base, override])}
        for probe, actual in zip(probes, response['snapshots']):
            selected = state[probe['target']]
            if probe['op'] == 'append':
                for key in probe['path']:
                    selected = selected[key]
                selected.append(probe['value'])
            else:
                for key in probe['path'][:-1]:
                    selected = selected[key]
                selected[probe['path'][-1]] = probe['value']
            self.assertEqual(actual, state)

    def test_empty(self):
        self.check_probes({}, {}, {}, [
            {'target': 'result', 'path': ['r'], 'op': 'set', 'value': 1},
            {'target': 'args', 'path': [0, 'b'], 'op': 'set', 'value': 2},
            {'target': 'args', 'path': [1, 'o'], 'op': 'set', 'value': 3},
        ])

    def test_flat(self):
        self.assertEqual(merge_config({'a': 1, 'b': 2}, {'a': 3}), {'a': 3, 'b': 2})

    def test_deep_regression(self):
        self.assertEqual(
            merge_config({'a': {'b': {'x': 1, 'y': 2}}}, {'a': {'b': {'x': 9}}}),
            {'a': {'b': {'x': 9, 'y': 2}}},
        )

    def test_falsey_overrides(self):
        for value in (None, False, 0, '', []):
            with self.subTest(value=value):
                self.assertEqual(merge_config({'k': {'old': 1}}, {'k': value}), {'k': value})

    def test_type_changes(self):
        self.assertEqual(
            merge_config({'a': 2, 'b': {'x': 1}, 'c': [1]},
                         {'a': {'z': 3}, 'b': [2], 'c': 'text'}),
            {'a': {'z': 3}, 'b': [2], 'c': 'text'},
        )

    def test_empty_dictionary_preserves_nested_keys(self):
        self.assertEqual(merge_config({'a': {'x': 1}}, {'a': {}}), {'a': {'x': 1}})

    def test_lists_replace(self):
        self.assertEqual(merge_config({'a': [1, 2]}, {'a': [3]}), {'a': [3]})

    def test_inputs_unchanged(self):
        base = {'a': {'x': [1]}, 'b': [2]}
        override = {'a': {'y': [3]}}
        response = call('config_merge', 'merge_config', [base, override])
        self.assertEqual(response['result'], {'a': {'x': [1], 'y': [3]}, 'b': [2]})
        self.assertEqual(response['args_after'], [base, override])

    def test_result_does_not_alias_inputs(self):
        base = {'keep': [{'deep': []}], 'nested': {'left': []}}
        override = {'new': [{'deep': []}], 'nested': {'right': []}}
        expected = {'keep': [{'deep': []}], 'new': [{'deep': []}],
                    'nested': {'left': [], 'right': []}}
        self.check_probes(base, override, expected, [
            {'target': 'result', 'path': ['keep', 0, 'deep'], 'op': 'append', 'value': 1},
            {'target': 'result', 'path': ['new', 0, 'deep'], 'op': 'append', 'value': 2},
            {'target': 'result', 'path': ['nested', 'left'], 'op': 'append', 'value': 3},
            {'target': 'result', 'path': ['nested', 'right'], 'op': 'append', 'value': 4},
        ])

    def test_inputs_do_not_alias_result(self):
        base = {'a': [{'x': []}]}
        override = {'b': {'c': []}}
        self.check_probes(base, override, {'a': [{'x': []}], 'b': {'c': []}}, [
            {'target': 'args', 'path': [0, 'a', 0, 'x'], 'op': 'append', 'value': 1},
            {'target': 'args', 'path': [1, 'b', 'c'], 'op': 'append', 'value': 2},
        ])


if __name__ == '__main__':
    unittest.main()

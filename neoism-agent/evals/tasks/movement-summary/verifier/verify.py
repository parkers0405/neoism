"""Trusted acceptance checks; candidate code runs only through JSON RPC."""
import copy
import unittest

from candidate_client import call


def summarize_movements(records, iterator=None):
    response = call('movements', 'summarize_movements', [records], iterator=iterator)
    if response['args_after'] != [records]:
        raise AssertionError('Candidate changed its input')
    return response['result']


class MovementAcceptance(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(summarize_movements([]), [])
        self.assertEqual(summarize_movements([], iterator='generator'), [])

    def test_single_record(self):
        self.assertEqual(summarize_movements([{'sku': 'A', 'delta': 7}]),
                         [{'sku': 'A', 'net': 7, 'movements': 1}])

    def test_signed_regression(self):
        self.assertEqual(summarize_movements([{'sku': 'A', 'delta': 5}, {'sku': 'A', 'delta': -8}]),
                         [{'sku': 'A', 'net': -3, 'movements': 2}])

    def test_zero_net_and_zero_delta(self):
        self.assertEqual(summarize_movements([{'sku': 'A', 'delta': d} for d in (4, -4, 0)]),
                         [{'sku': 'A', 'net': 0, 'movements': 3}])

    def test_sorted_interleaved_groups(self):
        records = [{'sku': s, 'delta': d} for s, d in [('b', 2), ('a', 1), ('b', -1), ('A', 4)]]
        self.assertEqual(summarize_movements(records), [
            {'sku': 'A', 'net': 4, 'movements': 1},
            {'sku': 'a', 'net': 1, 'movements': 1},
            {'sku': 'b', 'net': 1, 'movements': 2},
        ])

    def test_generator(self):
        records = [{'sku': 'x', 'delta': d} for d in (3, -1, 0, 6)]
        self.assertEqual(summarize_movements(records, iterator='generator'),
                         [{'sku': 'x', 'net': 8, 'movements': 4}])

    def test_single_iteration_incremental_reads(self):
        records = [{'sku': 'x', 'delta': d} for d in (2, -5, 4)]
        self.assertEqual(summarize_movements(records, iterator='single_pass_reused'),
                         [{'sku': 'x', 'net': 1, 'movements': 3}])

    def test_literal_skus_and_extra_fields(self):
        records = [{'sku': s, 'delta': 1, 'ignored': {'x': []}} for s in (' x ', '', 'x', 'é')]
        self.assertEqual(summarize_movements(records), [
            {'sku': '', 'net': 1, 'movements': 1},
            {'sku': ' x ', 'net': 1, 'movements': 1},
            {'sku': 'x', 'net': 1, 'movements': 1},
            {'sku': 'é', 'net': 1, 'movements': 1},
        ])

    def test_no_input_mutation_or_result_alias(self):
        records = [{'sku': 'x', 'delta': 3, 'extra': []}]
        probes = [
            {'target': 'result', 'path': [0, 'net'], 'op': 'set', 'value': 99},
            {'target': 'result', 'path': [0, 'sku'], 'op': 'set', 'value': 'changed'},
            {'target': 'args', 'path': [0, 0, 'delta'], 'op': 'set', 'value': 40},
            {'target': 'args', 'path': [0, 0, 'sku'], 'op': 'set', 'value': 'other'},
        ]
        response = call('movements', 'summarize_movements', [records], mutations=probes)
        expected = [{'sku': 'x', 'net': 3, 'movements': 1}]
        self.assertEqual(response['result'], expected)
        self.assertEqual(response['args_after'], [records])
        expected_args = copy.deepcopy([records])
        for index, probe in enumerate(probes):
            if probe['target'] == 'result':
                expected[0][probe['path'][-1]] = probe['value']
            else:
                expected_args[0][0][probe['path'][-1]] = probe['value']
            self.assertEqual(response['snapshots'][index],
                             {'result': expected, 'args': expected_args})

    def test_large_integer_and_independent_calls(self):
        big = 10 ** 40
        self.assertEqual(summarize_movements([{'sku': 'x', 'delta': big}, {'sku': 'x', 'delta': -1}]),
                         [{'sku': 'x', 'net': big - 1, 'movements': 2}])
        self.assertEqual(summarize_movements([]), [])
        self.assertEqual(summarize_movements([{'sku': 'x', 'delta': 0}]),
                         [{'sku': 'x', 'net': 0, 'movements': 1}])


if __name__ == '__main__':
    unittest.main()

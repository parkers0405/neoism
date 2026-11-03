import importlib.util
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("neoism_profiler", Path(__file__).with_name("profile-neoism-processes.py"))
profiler = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = profiler
SPEC.loader.exec_module(profiler)


def process(pid, ppid, role="other", start="Mon Nov 2 16:51:14 2026", rss=100, threads=2):
    return profiler.Process(pid, ppid, start, threads, rss, role)


class OwnershipTests(unittest.TestCase):
    def test_active_old_roots_unattached_and_generic_node(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10, "workspace_server"),
                        process(12, 11, "node_runtime"), process(20, 1, "frontend"),
                        process(21, 20, "rust_analyzer"), process(30, 1, "agent_server"),
                        process(31, 1, "node_runtime"), process(32, 1, "typescript_language_server")])
        own = lambda pid: history.ownership(history.latest[pid])
        self.assertEqual(own(12), ("active_frontend", 10))
        self.assertEqual(own(21), ("other_neoism_tree", 20))
        self.assertEqual(own(30), ("other_neoism_tree", 30))
        self.assertIsNone(own(31))
        self.assertEqual(own(32), ("unattached_candidate", None))

    def test_orphan_server_roots_include_generic_node_and_tsserver(self):
        for role in ("workspace_server", "agent_server"):
            with self.subTest(root_role=role):
                history = profiler.History(10, 90)
                history.add(0, [process(10, 1, "frontend"), process(20, 1, role),
                                process(21, 20, "node_runtime"), process(22, 21, "tsserver"),
                                process(30, 1, "node_runtime")])
                for pid in (20, 21, 22):
                    self.assertEqual(history.ownership(history.latest[pid]), ("other_neoism_tree", 20))
                self.assertIsNone(history.ownership(history.latest[30]))
                # Reparenting after the daemon exits retains the observed tree.
                history.add(2, [process(21, 1, "node_runtime"), process(22, 21, "tsserver")])
                self.assertEqual(history.ownership(history.latest[22]), ("other_neoism_tree", 20))

    def test_outer_root_groups_nested_servers_and_active_frontend_wins(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10, "workspace_server"),
                        process(12, 11, "agent_server"), process(13, 12, "node_runtime"),
                        process(20, 1, "frontend"), process(21, 20, "workspace_server"),
                        process(22, 21, "agent_server"), process(23, 22, "tsserver")])
        self.assertEqual(history.ownership(history.latest[13]), ("active_frontend", 10))
        for pid in (20, 21, 22, 23):
            self.assertEqual(history.ownership(history.latest[pid]), ("other_neoism_tree", 20))

    def test_profiler_descendants_excluded_even_inside_active_tree(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(90, 10), process(91, 90, "other_tool"), process(92, 91, "cargo")])
        self.assertIsNone(history.ownership(history.latest[90]))
        self.assertIsNone(history.ownership(history.latest[92]))

    def test_exited_parent_ancestry_survives_reparenting(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10), process(12, 11, "rustc")])
        history.add(2, [process(12, 1, "rustc")])
        self.assertEqual(history.ownership(history.latest[12]), ("active_frontend", 10))
        self.assertEqual(history.at(11, 2), history.latest[11])

    def test_reused_pid_is_not_attributed_to_old_tree(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10, "cargo")])
        history.add(2, [process(11, 1, "node_runtime", start="new start")])
        self.assertEqual(history.reused, {11})
        self.assertIsNone(history.at(11, 1))
        self.assertIsNone(history.ownership(history.latest[11]))


class ParsingTests(unittest.TestCase):
    def test_bun_source_frontend_requires_distinctive_preload_and_entrypoint(self):
        positives = [
            "bun --preload ./scripts/display-binding-preload.ts src/main.tsx --workspace example",
            "/nix/store/example-bun/bin/bun --preload ./scripts/display-binding-preload.ts src/main.tsx --workspace example",
            "bun --preload=/project/scripts/display-binding-preload.ts /project/src/main.tsx",
            "bun --preload './project with spaces/scripts/display-binding-preload.ts' ./src/main.tsx",
        ]
        negatives = [
            "bun src/main.tsx",
            "node --preload ./scripts/display-binding-preload.ts src/main.tsx",
            "bun --preload ./scripts/other-preload.ts src/main.tsx",
            "bun --preload ./display-binding-preload.ts src/main.tsx",
            "bun --preload ./scripts/display-binding-preload.ts other/main.tsx",
            "bun --preload ./scripts/display-binding-preload.ts src/main.tsx.bak",
            "bun --preload ./scripts/display-binding-preload.ts.bak src/main.tsx",
            "bun server.ts --preload ./scripts/display-binding-preload.ts src/main.tsx",
            "bun src/main.tsx --preload ./scripts/display-binding-preload.ts",
            "bun --preload ./scripts/display-binding-preload.ts --eval src/main.tsx",
        ]
        for args in positives:
            with self.subTest(args=args):
                self.assertEqual(profiler.classify("bun", args), "frontend")
        for args in negatives:
            with self.subTest(args=args):
                self.assertEqual(profiler.classify("bun", args), "node_runtime")

    def test_bun_frontend_role_keeps_active_root_ancestry_priority(self):
        role = profiler.classify("bun", "bun --preload ./scripts/display-binding-preload.ts src/main.tsx")
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10, role), process(12, 11, "node_runtime")])
        for pid in (11, 12):
            self.assertEqual(history.ownership(history.latest[pid]), ("active_frontend", 10))

    def test_roles(self):
        cases = {
            "/usr/bin/neoism": "frontend",
            "/usr/bin/neoism --daemon": "workspace_server",
            "/usr/bin/neoism --neoism-internal-workspace-daemon --workspace /private": "workspace_server",
            "/usr/bin/neoism --neoism-internal-agent-server": "agent_server",
            "/usr/bin/neoism --neoism-internal-workspace-daemon-extra": "frontend",
            "/usr/bin/neoism --label=--neoism-internal-agent-server": "frontend",
            "/usr/bin/neoism --label=--agent-server": "frontend",
            "/tmp/neoism-workspace-daemon --workspace /private": "workspace_server",
            "/tmp/neoism-agent-server": "agent_server",
            "node /nix/lib/typescript-language-server/lib/cli.mjs --stdio": "typescript_language_server",
            "node /nix/typescript/lib/tsserver.js --stdio": "tsserver",
            "node /nix/typescript/lib/tsserver.js --globalPlugins typescript-language-server": "tsserver",
            "node /nix/typescript-language-server/node_modules/typescript/lib/tsserver.js": "tsserver",
            "node /nix/typescript/lib/typingsInstaller.js --globalPlugins typescript-language-server": "typings_installer",
            "node --max-old-space-size=4096 /nix/typescript/lib/tsserver.js": "tsserver",
            "node /nix/typescript/lib/typingsInstaller.js": "typings_installer",
            "node /nix/yaml-language-server/bin/yaml-language-server --stdio": "yaml_language_server",
            "node /private/server.js": "node_runtime",
            "node /private/typescript/not-a-language-server.js": "node_runtime",
            "node /private/server.js --label typescript-language-server": "node_runtime",
            "/nix/bin/rust-analyzer": "rust_analyzer",
            "/nix/bin/cargo check": "cargo",
            "/nix/bin/rustc --crate-name private": "rustc",
            "/nix/bin/clang -c private.c": "other_tool",
        }
        for args, role in cases.items():
            with self.subTest(args=args):
                self.assertEqual(profiler.classify("truncated-comm", args), role)

    def test_ps_private_args_not_retained(self):
        rows = profiler.parse_ps(" 10 1 Mon Nov  2 16:51:14 2026 8 500 neoism /bin/neoism --secret=PRIVATE\n")
        self.assertEqual(rows[0].threads, 8)
        self.assertEqual(rows[0].rss, 500)
        self.assertNotIn("PRIVATE", repr(rows))
        self.assertEqual(rows[0].role, "frontend")

    def test_structured_schema_and_bad_data(self):
        text = '{"sysstat":{"hosts":[{"statistics":[{"timestamp":"12:00:00","task-cpu-load":[{"PID":"10","cpu":20}]}]}]}}'
        stats, invalid = profiler.parse_pidstat(text, 1)
        self.assertFalse(invalid)
        self.assertEqual(stats[0]["task-cpu-load"][0]["cpu"], 20)
        self.assertEqual(profiler.parse_pidstat("not json", 1), ([], True))
        self.assertIsNone(profiler.number("nan"))
        self.assertIsNone(profiler.number(-1))

    def test_pss_unsupported_is_unknown_and_query_is_targeted(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(30, 1, "node_runtime")])
        with patch.object(profiler, "capture", return_value=("", 1, False)) as capture:
            values, status = profiler.pss_endpoint(history, "/bin/ps")
        self.assertEqual(values, {})
        self.assertTrue(status["query_failed_or_unsupported"])
        self.assertEqual(capture.call_args.args[0], ["/bin/ps", "-p", "10", "-o", "pid=,pss="])

    def test_pipe_drains_while_caller_is_not_reading(self):
        with patch.object(profiler, "MAX_BYTES", 100000):
            collector = profiler.Collector([sys.executable, "-c", "print('x' * 300000)"])
            # wait() before finish() would deadlock with an undrained pipe.
            collector.process.wait(timeout=5)
            text, code, timed_out = collector.finish(1)
        self.assertEqual(len(text), 100000)
        self.assertTrue(collector.truncated)
        self.assertEqual(code, 0)
        self.assertFalse(timed_out)

    def test_missing_pidstat_explains_explicit_path_without_sampling(self):
        for extra_args in ([], ["--pidstat", "/missing/pidstat"]):
            with self.subTest(extra_args=extra_args):
                output = io.StringIO()
                with patch.object(profiler.shutil, "which", return_value=None) as which, \
                        patch.object(profiler, "capture") as capture, redirect_stdout(output):
                    code = profiler.main(["--root-pid", "10"] + extra_args)
                self.assertEqual(code, 1)
                report = json.loads(output.getvalue())
                self.assertEqual(report["error"], "pidstat_unavailable")
                self.assertIn("--pidstat /path/to/pidstat", report["message"])
                self.assertIn("PATH", report["message"])
                which.assert_any_call(extra_args[1] if extra_args else "pidstat")
                capture.assert_not_called()


class MathTests(unittest.TestCase):
    def history(self):
        history = profiler.History(10, 90)
        history.add(0, [process(10, 1, "frontend"), process(11, 10, "rustc"), process(12, 10, "rustc")])
        history.measurements = [[(p.key, p.rss, p.threads) for p in history.records.values()]]
        return history

    def test_period_mean_group_simultaneous_peak_and_missing_io(self):
        history = self.history()
        statistics = [
            {"task-cpu-load": [{"PID": 11, "cpu": 100, "wait": 5}, {"PID": 12, "cpu": 0}],
             "task-memory": [{"PID": 11, "RSS": 500, "majflt/s": 2}, {"PID": 12, "RSS": 100}],
             "io": [{"PID": 11, "kB_rd/s": 4, "kB_wr/s": 0}],
             "context-switch": [{"PID": 11, "cswch/s": 8, "nvcswch/s": 1}]},
            {"task-cpu-load": [{"PID": 11, "cpu": 0}, {"PID": 12, "cpu": 80}],
             "task-memory": [{"PID": 11, "RSS": 100}, {"PID": 12, "RSS": 600}]},
        ]
        report = profiler.summarize(history, statistics, 2, 2, {history.latest[11]: 50}, {})
        rows = {row["pid"]: row for row in report["processes"]}
        self.assertEqual(rows[11]["rates"]["cpu_percent"]["period_mean"], 50)
        self.assertEqual(rows[11]["rates"]["read_kib_s"]["period_mean"], 2)
        self.assertEqual(rows[11]["rates"]["read_kib_s"]["known_intervals"], 1)
        self.assertIsNone(rows[12]["rates"]["read_kib_s"]["period_mean"])
        self.assertEqual(rows[11]["rates"]["write_kib_s"]["peak"], 0)
        self.assertIsNone(rows[10]["pss_start_kib"])
        group = next(row for row in report["groups"] if row["role"] == "rustc")
        self.assertEqual(group["rates"]["cpu_percent"]["period_mean"], 90)
        self.assertEqual(group["rates"]["cpu_percent"]["peak"], 100)
        self.assertEqual(group["rss_peak_kib"], 700)  # NOT 500 + 600.
        self.assertEqual(group["threads_peak"], 4)
        self.assertEqual(group["rates"]["read_kib_s"]["unknown_process_intervals"], 3)
        self.assertEqual(group["pss_start_kib"], {"known_sum": 50, "known_processes": 1, "unknown_processes": 1})

    def test_missing_whole_interval_and_no_data(self):
        history = self.history()
        report = profiler.summarize(history, [{"task-cpu-load": [{"PID": 11, "cpu": 100}]}], 2, 2, {}, {})
        row = next(row for row in report["processes"] if row["pid"] == 11)
        self.assertEqual(row["rates"]["cpu_percent"]["period_mean"], 50)
        self.assertEqual(row["rates"]["cpu_percent"]["known_intervals"], 1)
        self.assertIsNone(row["rates"]["major_faults_s"]["peak"])


if __name__ == "__main__":
    unittest.main()
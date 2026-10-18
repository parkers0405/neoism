"""Credential-free checks of the actual agent-phase network and repository."""

import json
from pathlib import Path
import re
import shlex

from harbor.agents.base import BaseAgent


class ProtocolProbe(BaseAgent):
    @staticmethod
    def name():
        return "hard51-protocol-probe"

    def version(self):
        return "1"

    async def setup(self, environment):
        pass

    async def run(self, instruction, environment, context):
        policy = environment.network_policy
        if policy.network_mode.value != "allowlist" or set(policy.allowed_hosts) != {"api.anthropic.com"}:
            raise RuntimeError("Agent-phase allowlist was not applied")
        if not environment.capabilities.dynamic_network_policy or not environment.capabilities.network_allowlist:
            raise RuntimeError("Probe backend cannot enforce phase-specific allowlists")
        config = json.loads((self.logs_dir.parent / "config.json").read_text())
        task = Path(config["task"]["path"])
        match = re.search(r"-([0-9a-f]{40})", task.name)
        if match is None:
            raise RuntimeError("Official task lacks fix-commit identity")
        lines = (task / "tests/test_patch.patch").read_text().splitlines()
        created = [lines[i + 1][6:] for i, line in enumerate(lines[:-1])
                   if line.startswith("--- /dev/null") and lines[i + 1].startswith("+++ b/")]
        created += [line[len("rename to "):] for line in lines if line.startswith("rename to ")]
        if any(Path(name).is_absolute() or ".." in Path(name).parts for name in created):
            raise RuntimeError("Unsafe upstream hidden-test path")
        probe_spec = {"fix_sha": match.group(1), "created_files": created}
        script = r'''
import json, pathlib, subprocess, sys, urllib.request, urllib.error
spec=json.loads(sys.argv[1])
opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
results={}
for url in ('https://api.anthropic.com/v1/models','https://api.github.com','https://pypi.org/simple/','https://registry.npmjs.org'):
    try:
        with opener.open(url, timeout=6) as response:
            results[url]={'reachable':True,'status':response.status}
    except urllib.error.HTTPError as error:
        results[url]={'reachable':True,'status':error.code}
    except (OSError,urllib.error.URLError):
        results[url]={'reachable':False}
def git(*args):
    p=subprocess.run(['git','-c','core.hooksPath=/dev/null','-c','core.fsmonitor=false']+list(args),capture_output=True,text=True,timeout=180)
    return {'code':p.returncode,'output':p.stdout.strip()}
repo=git('-C','/app','rev-parse','--show-toplevel')
if repo['code']:
    repo=git('-C','/testbed','rev-parse','--show-toplevel')
root=repo['output']
report={'network':results,'repository':root,'head_commits':git('-C',root,'rev-list','HEAD','--count'),
        'all_commits':git('-C',root,'rev-list','--all','--count'),
        'reflog':git('-C',root,'reflog'),'stash':git('-C',root,'stash','list'),
        'fix_object':git('-C',root,'cat-file','-t',spec['fix_sha']),
        'fsck':git('-C',root,'fsck','--unreachable','--no-reflogs'),
        'hidden_directories':[str(p) for p in (pathlib.Path('/tests'),pathlib.Path('/solution')) if p.exists()],
        'hidden_test_files':[name for name in spec['created_files'] if (pathlib.Path(root)/name).exists()],
        'historical_heads':[name for name in ('ORIG_HEAD','FETCH_HEAD') if (pathlib.Path(root)/'.git'/name).exists()],
        'non_sample_hooks':[p.name for p in (pathlib.Path(root)/'.git/hooks').glob('*') if not p.name.endswith('.sample')]}
print(json.dumps(report))
'''
        result = await environment.exec("python3 -c " + shlex.quote(script) + " " + shlex.quote(json.dumps(probe_spec)), timeout_sec=240)
        if result.return_code:
            raise RuntimeError("Credential-free protocol probe did not execute")
        report = json.loads(result.stdout)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        (self.logs_dir / "protocol-probe.json").write_text(json.dumps(report, indent=2) + "\n")
        context.metadata = {"protocol_probe": report, "inference_calls": 0, "credentials_read": False}
        network = report["network"]
        if not network["https://api.anthropic.com/v1/models"]["reachable"]:
            raise RuntimeError("Allowed model endpoint is not reachable")
        if any(item["reachable"] for url, item in network.items() if "api.anthropic.com" not in url):
            raise RuntimeError("Forbidden agent-phase network destination is reachable")
        if (not report["repository"] or report["repository"] not in ("/app", "/testbed") or
                 any(report[key]["code"] for key in ("head_commits", "all_commits", "reflog", "stash", "fsck"))):
            raise RuntimeError("Repository sanitation probe failed to inspect git state")
        if (report["head_commits"]["output"] != "1" or report["all_commits"]["output"] not in ("0", "1")
                or report["reflog"]["output"] or report["stash"]["output"]
                or report["fix_object"]["code"] == 0
                or any("commit" in line for line in report["fsck"]["output"].splitlines())
                or any(report[key] for key in ("hidden_directories", "hidden_test_files", "historical_heads", "non_sample_hooks"))):
            raise RuntimeError("Repository history, fix object, or hidden verifier files are not sanitized")


class InstallOnlyAgent(BaseAgent):
    """Exercise real frozen installation for all three, never their run methods."""

    def __init__(self, *args, harnesses, **kwargs):
        super().__init__(*args, **kwargs)
        self.harnesses = harnesses

    @staticmethod
    def name():
        return "hard51-install-only"

    def version(self):
        return "1"

    async def setup(self, environment):
        import importlib
        self.installed_agents = {}
        for name, spec in self.harnesses.items():
            module, klass = spec["import_path"].split(":")
            agent = getattr(importlib.import_module(module), klass)(
                logs_dir=self.logs_dir / name, **spec["kwargs"])
            await agent.install(environment)
            self.installed_agents[name] = agent

    async def run(self, instruction, environment, context):
        report = {}
        for name, spec in self.harnesses.items():
            agent = self.installed_agents[name]
            binary = await agent.runtime_binary()
            result = await environment.exec(command=shlex.quote(binary) + " --version", timeout_sec=120,
                                            env={"HOME": "/tmp/neoism-install-only", "XDG_CONFIG_HOME": "/tmp/neoism-install-only/config"})
            output = (result.stdout or "") + (result.stderr or "")
            report[name] = {"return_code": result.return_code, "output": output[-3000:]}
            if result.return_code or not re.search(r"(?<![\d.])" + re.escape(spec["kwargs"]["version"]) + r"(?![\d.])", output):
                raise RuntimeError("Frozen install/version check failed: " + name)
        self.logs_dir.mkdir(parents=True, exist_ok=True)
        (self.logs_dir / "install-only.json").write_text(json.dumps(report, indent=2) + "\n")
        context.metadata = {"install_only": report, "inference_calls": 0, "credentials_read": False}


# Use Harbor's actual oracle semantics (execute solution/solve.sh), not a gold
# patch approximation. Capture only afterwards for a separate pristine replay.
from harbor.agents.oracle import OracleAgent


class CaptureOracleAgent(OracleAgent):
    def __init__(self, logs_dir, task_dir, **kwargs):
        from harbor.models.trial.paths import TrialPaths
        super().__init__(logs_dir=logs_dir, task_dir=Path(task_dir),
                         trial_paths=TrialPaths(trial_dir=Path(logs_dir).parent), **kwargs)

    async def run(self, instruction, environment, context):
        await super().run(instruction, environment, context)
        from neoism_harbor.common import _CAPTURE
        result = await environment.exec(command=_CAPTURE, user="root", timeout_sec=300)
        if result.return_code:
            raise RuntimeError("Oracle patch capture failed")

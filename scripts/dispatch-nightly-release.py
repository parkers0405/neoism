#!/usr/bin/env python3
"""Keep nightly caches on one branch while building an exact immutable source."""
import json
import os
import re
import subprocess

CACHE_BRANCH = 'nightly-builds'


def dispatch(repo, tag, sha):
    if not re.fullmatch(r'v\d+\.\d+\.\d+-nightly\.[A-Za-z0-9.]+', tag):
        raise ValueError('Only nightly releases may use the shared cache branch')
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('An exact source commit is required')
    ref = f'repos/{repo}/git/ref/heads/{CACHE_BRANCH}'
    result = subprocess.run(['gh', 'api', ref], text=True, capture_output=True)
    if result.returncode == 0:
        previous = json.loads(result.stdout)['object']['sha']
        if previous != sha:
            # Never force an existing branch backwards or across divergent work.
            subprocess.run(['gh', 'api', '--method', 'PATCH',
                f'repos/{repo}/git/refs/heads/{CACHE_BRANCH}',
                '-f', f'sha={sha}', '-F', 'force=false'], check=True)
    elif '404' in result.stderr:
        subprocess.run(['gh', 'api', '--method', 'POST', f'repos/{repo}/git/refs',
            '-f', f'ref=refs/heads/{CACHE_BRANCH}', '-f', f'sha={sha}'], check=True)
    else:
        raise RuntimeError(result.stderr)
    subprocess.run(['gh', 'workflow', 'run', 'release-neoism.yml', '--repo', repo,
        '--ref', CACHE_BRANCH, '-f', f'release_tag={tag}', '-f', f'source_ref={sha}',
        '-f', 'platform=all'], check=True)
    print(f'Dispatched {tag} at {sha}; reusable cache scope: {CACHE_BRANCH}')


if __name__ == '__main__':
    dispatch(os.environ['GH_REPO'], os.environ['GITHUB_REF_NAME'], os.environ['GITHUB_SHA'])
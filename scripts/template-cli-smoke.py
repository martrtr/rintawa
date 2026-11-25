#!/usr/bin/env python3
"""Offline CLI: explicit template selection, no grants on install, and independent forks."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

binary = Path(os.environ['RINTAWA_BINARY']).resolve()
artifacts = Path(os.environ['RINTAWA_TEMPLATE_ARTIFACTS']).resolve()
output = Path(os.environ.get('RINTAWA_TEMPLATE_EVIDENCE', tempfile.gettempdir()))
output.mkdir(parents=True, exist_ok=True)
root = Path(tempfile.mkdtemp(prefix='template-cli-', dir=output))
commands = []


def cli(home, *args, success=True):
    argv = [str(binary), '--home', str(home), *map(str, args)]
    result = subprocess.run(argv, capture_output=True, text=True, timeout=30)
    commands.append({'argv': argv, 'exit_status': result.returncode,
                     'stdout': result.stdout, 'stderr': result.stderr})
    assert (result.returncode == 0) == success, commands[-1]
    return result.stdout


def baseline(home):
    result = cli(home, 'install', artifacts / 'web-runtime.rtw')
    instance = re.search(r'^instance = (.+)$', result, re.M)[1]
    for permission in ['background-task', 'loopback-listen']:
        cli(home, 'grant-runtime', instance, 'runtime', permission)


def profile(home, world):
    return tomllib.loads((home / 'worlds' / world / 'composition.toml').read_text())


def create(home, digest, title):
    result = cli(home, 'world-create', '--template', digest, '--title', title,
                 '--approve-runtime-permissions')
    return re.search(r'^world = (.+)$', result, re.M)[1]


try:
    home = root / 'source'
    baseline(home)
    installed = cli(home, 'package-install', artifacts / 'TextRoleplay.rtw')
    digest = re.search(r'^digest = (.+)$', installed, re.M)[1]
    info = json.loads(cli(home, 'template-info', digest))
    assert info['name'] == 'Text Roleplay'
    assert len(info['components']) == 2 and len(info['hostRequirements']) == 1
    assert len(tomllib.loads((home / 'profiles/baseline.toml').read_text())['activations']) == 1
    cli(home, 'world-create', '--template', digest, success=False)
    assert not list((home / 'worlds').iterdir())
    a, b = create(home, digest, 'Story A'), create(home, digest, 'Story B')
    assert a != b
    pa, pb = profile(home, a), profile(home, b)
    assert not ({row['instance-id'] for row in pa['activations']} &
                {row['instance-id'] for row in pb['activations']})
    assert pa['execution_requirements'] == pb['execution_requirements']
    assert pa['entry']['instance-id'] != pb['entry']['instance-id']
    exported = root / 'exported.rtw'
    cli(home, 'world-export', a, exported)
    cli(home, 'world-export', a, exported, success=False)
    clean = root / 'clean'
    baseline(clean)
    installed = cli(clean, 'package-install', exported)
    fork_digest = re.search(r'^digest = (.+)$', installed, re.M)[1]
    cli(clean, 'world-create', '--template', fork_digest, success=False)
    c, d = create(clean, fork_digest, 'Fork C'), create(clean, fork_digest, 'Fork D')
    assert len({a, b, c, d}) == 4
    for world in [c, d]:
        row = profile(clean, world)
        slot_subjects = {slot['slot']: slot['packageId'] for slot in info['components']}
        expected_grants = {(slot_subjects[grant['slot']], grant['componentId'], grant['permission'])
                           for grant in info['composition']['requestedGrants']}
        instances = {activation['instance-id']: activation['subject'] for activation in row['activations']}
        actual_grants = {(instances[grant['instance-id']], grant['component-id'], grant['permission'])
                         for grant in row['runtime_permissions']}
        assert actual_grants == expected_grants
        assert all(grant['scope-id'] == 'world:' + world for grant in row['runtime_permissions'])
        assert 'preferences' not in row and 'http_loopback_origins' not in row
        assert not ({item['instance-id'] for item in row['activations']} &
                    {item['instance-id'] for item in pa['activations']})
    checks = ['install does not activate domain packages or create a World',
              'approval is required explicitly and refused creation publishes nothing',
              'template selection precedes independent DB/composition publication',
              'self-contained export installs in a clean home without carried approvals',
              'four fresh World IDs and disjoint runtime identities, preserved exact prerequisites',
              'existing export output is never replaced']
    (root / 'result.json').write_text(json.dumps({'status': 'passed', 'checks': checks}, indent=2))
    print(json.dumps({'status': 'passed', 'evidence': str(root), 'checks': checks}))
finally:
    (root / 'commands.json').write_text(json.dumps(commands, indent=2))

"""Actual CPA executable smoke test, isolated fake OAuth and no upstream calls.

Set CPA_CORE_EXE to the existing executable. Requires already installed PyYAML.
An intentionally invalid threshold blocks before any quota HTTP request.
"""
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

import yaml

exe = os.environ['CPA_CORE_EXE']
project = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix='cpa-guard-core-') as tmp:
    root = Path(tmp)
    (root/'auth').mkdir()
    (root/'plugins').mkdir()
    auth = root/'auth/codex-fixture.json'
    document = {'type': 'codex', 'access_token': 'fixture-not-real', 'account_id': 'fixture',
                'email': 'fixture@example.invalid', 'plan_type': 'plus', 'disabled': False}
    auth.write_text(json.dumps(document), encoding='utf-8')
    shutil.copy2(project/'target/quota-guard/release/codex_window_activation.dll',
                 root/'plugins/codex-quota-guard-v0.2.0.dll')
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        port = sock.getsockname()[1]
    cfg = {'host': '127.0.0.1', 'port': port, 'auth-dir': str(root/'auth'),
           'api-keys': ['fixture-client'], 'request-retry': 0,
           'routing': {'strategy': 'round-robin', 'session-affinity': True},
           'plugins': {'enabled': True, 'dir': str(root/'plugins'),
                       'configs': {'codex-quota-guard': {'enabled': True}}}}
    path = root/'config.yaml'
    path.write_text(yaml.safe_dump(cfg), encoding='utf-8')
    headers = {'Authorization': 'Bearer fixture-client', 'Content-Type': 'application/json',
               'X-Session-Affinity': 'fixture-session'}
    with (root/'log.txt').open('w', encoding='utf-8') as log:
        proc = subprocess.Popen([exe, '-config', str(path)], cwd=root, stdout=log, stderr=log,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            names = []
            for _ in range(100):
                try:
                    request = urllib.request.Request(f'http://127.0.0.1:{port}/v1/models', headers=headers)
                    with urllib.request.urlopen(request, timeout=.3) as response:
                        names = [m['id'] for m in json.load(response)['data']]
                    if names:
                        break
                except (OSError, urllib.error.URLError):
                    pass
                time.sleep(.1)
            assert names, 'No fake Codex models loaded'
            model = 'gpt-6.1-sol' if 'gpt-6.1-sol' in names else names[0]
            document['codex_quota_guard_limit'] = 101
            auth.write_text(json.dumps(document), encoding='utf-8')
            request = urllib.request.Request(f'http://127.0.0.1:{port}/v1/responses',
                        data=json.dumps({'model': model, 'input': 'fixture'}).encode(), headers=headers)
            # Let the auth-file watcher and first guard poll observe the policy.
            # The request-time guard also reads the physical JSON independently.
            time.sleep(1)
            try:
                urllib.request.urlopen(request, timeout=10)
                raise AssertionError('Guard did not veto')
            except urllib.error.HTTPError as error:
                body = json.loads(error.read())
                # Either the explicit after-selection veto or the already-paused
                # empty pool is safe. Neither may reach the OAuth upstream.
                assert error.code >= 400
                if error.code == 429:
                    assert body['error']['code'] == 'codex_quota_guard', body
            persisted = json.loads(auth.read_text(encoding='utf-8'))
            assert persisted['disabled'] and persisted['codex_quota_guard_paused']
            try:
                urllib.request.urlopen(request, timeout=10)
                raise AssertionError('Disabled pool was routed')
            except urllib.error.HTTPError as error:
                assert error.code >= 400
            print('PASS real CPA: registered native guard, invalid policy stops without quota HTTP, '
                  'exact credential persisted disabled, empty eligible pool fails')
        finally:
            proc.terminate()
            proc.wait(timeout=10)

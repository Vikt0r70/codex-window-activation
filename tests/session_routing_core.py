"""Native session distribution with guard installed; only local mock traffic.

Set CPA_CORE_EXE to an existing core. Set CPA_SLEEV_URL optionally to test
the running Sleev forwarding boundary too. Never uses real OAuth credentials.
"""
import http.server
from collections import Counter
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request

import yaml

seen = []


class Mock(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        credential = self.headers['Authorization'].removeprefix('Bearer ')
        seen.append(credential)
        body = json.dumps({'id': 'fixture', 'object': 'chat.completion', 'model': req['model'],
                          'choices': [{'index': 0, 'message': {'role': 'assistant', 'content': credential},
                                       'finish_reason': 'stop'}],
                          'usage': {'prompt_tokens': 1, 'completion_tokens': 1, 'total_tokens': 2}}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Mock)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    with tempfile.TemporaryDirectory(prefix='cpa-routing-core-') as tmp:
        root = Path(tmp)
        (root/'auth').mkdir()
        (root/'plugins').mkdir()
        project = Path(__file__).resolve().parents[1]
        shutil.copy2(project/'target/quota-guard/release/codex_window_activation.dll',
                     root/'plugins/codex-quota-guard-v0.2.0.dll')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        pool = {'name': 'fixture-pool', 'base-url': f'http://127.0.0.1:{server.server_port}/v1',
                'api-key-entries': [{'api-key': key} for key in ['fixture-a', 'fixture-b', 'fixture-c']],
                'models': [{'name': 'fixture-model', 'alias': 'fixture-model'}]}
        weighted = os.environ.get('CPA_WEIGHTED_TEST') == '1'
        if weighted:
            for entry, weight in zip(pool['api-key-entries'], [1, 3, 1]):
                entry['weight'] = weight
        cfg = {'host': '127.0.0.1', 'port': port, 'auth-dir': str(root/'auth'),
               'api-keys': ['same-client-key'], 'request-retry': 0,
               'routing': {'strategy': 'weighted-round-robin' if weighted else 'round-robin', 'session-affinity': True,
                           'session-affinity-ttl': '24h', 'session-affinity-subagents': True},
               'openai-compatibility': [pool],
               'plugins': {'enabled': True, 'dir': str(root/'plugins'),
                           'configs': {'codex-quota-guard': {'enabled': True}}}}
        path = root/'config.yaml'

        def save():
            path.write_text(yaml.safe_dump(cfg), encoding='utf-8')

        save()
        core = f'http://127.0.0.1:{port}/v1'
        headers = {'Authorization': 'Bearer same-client-key', 'Content-Type': 'application/json'}
        gateway = os.environ.get('CPA_SLEEV_URL')
        url = gateway.rstrip('/') if gateway else core
        if gateway:
            headers.update({'sleev-harness': 'opencode', 'sleev-base-url': core})
        with (root/'log.txt').open('w', encoding='utf-8') as log:
            proc = subprocess.Popen([os.environ['CPA_CORE_EXE'], '-config', str(path)], cwd=root,
                                    stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
            try:
                for _ in range(100):
                    try:
                        with urllib.request.urlopen(urllib.request.Request(url+'/models', headers=headers), timeout=.5) as r:
                            if any(m['id'] == 'fixture-model' for m in json.load(r)['data']):
                                break
                    except (OSError, urllib.error.URLError):
                        pass
                    time.sleep(.1)
                else:
                    raise AssertionError('Fixture catalog not ready')

                def call(session, parent=None):
                    h = {**headers, 'X-Session-Affinity': session, 'X-OpenCode-Session-Id': session}
                    if parent:
                        h['X-Parent-Session-Id'] = parent
                    body = {'model': 'fixture-model', 'messages': [{'role': 'user', 'content': 'fixture'}]}
                    with urllib.request.urlopen(urllib.request.Request(url+'/chat/completions',
                            data=json.dumps(body).encode(), headers=h), timeout=10) as r:
                        return json.load(r)['choices'][0]['message']['content']

                sessions = ['conversation-one', 'conversation-two', 'conversation-three']
                bindings = {s: call(s) for s in sessions}
                if weighted:
                    sample = Counter(call('weighted-new-'+str(i)) for i in range(30))
                    assert sample == Counter({'fixture-a': 6, 'fixture-b': 18, 'fixture-c': 6}), sample
                    for entry in pool['api-key-entries']:
                        entry['weight'] = 3 if entry['api-key'] == 'fixture-a' else 1
                    save()
                    time.sleep(1)
                    assert all(call(s) == bindings[s] for s in sessions), 'Weight update moved warm sessions'
                else:
                    assert len(set(bindings.values())) == 3, bindings
                for s in sessions:
                    assert call(s) == bindings[s], 'Repeat turn lost account binding'
                assert call('child-session', sessions[0]) == bindings[sessions[0]], 'Child lost parent affinity'
                removed = bindings[sessions[0]]
                pool['api-key-entries'] = [k for k in pool['api-key-entries'] if k['api-key'] != removed]
                save()
                time.sleep(1)
                assert call(sessions[0]) != removed, 'Unavailable binding failed to move'
                for s in sessions[1:]:
                    result = call(s)
                    if bindings[s] == removed:
                        assert result != removed, 'Affected session failed to move'
                    else:
                        assert result == bindings[s], 'Unrelated session moved'
                cfg['openai-compatibility'] = []
                save()
                time.sleep(1)
                before = len(seen)
                try:
                    call(sessions[0])
                    raise AssertionError('Empty pool accepted request')
                except urllib.error.HTTPError as error:
                    assert error.code >= 400
                assert len(seen) == before, 'Empty pool sent an upstream request'
                print('PASS actual CPA + guard' + (' + Sleev' if gateway else '') +
                      (': weighted 6/18/6 distribution and warm weight-update affinity, ' if weighted else ': 3 sessions spread, ') +
                      'shared client key, repeat affinity, child affinity, '
                      'only unavailable session moves, empty pool stops; zero real quota')
            finally:
                proc.terminate()
                proc.wait(timeout=10)
finally:
    server.shutdown()
    server.server_close()
    thread.join()

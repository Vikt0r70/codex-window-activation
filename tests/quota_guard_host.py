"""Exercise the real guard DLL against a controlled CPA host, no real quota."""
import base64
import ctypes as c
import json
import os
from pathlib import Path
import tempfile
import time

class Buffer(c.Structure):
    _fields_ = [('ptr', c.c_void_p), ('len', c.c_size_t)]
HostCall = c.CFUNCTYPE(c.c_int32, c.c_void_p, c.c_char_p, c.c_void_p, c.c_size_t, c.POINTER(Buffer))
Free = c.CFUNCTYPE(None, c.c_void_p, c.c_size_t)
PluginCall = c.CFUNCTYPE(c.c_int32, c.c_char_p, c.c_void_p, c.c_size_t, c.POINTER(Buffer))
Shutdown = c.CFUNCTYPE(None)
class Host(c.Structure):
    _fields_ = [('version', c.c_uint32), ('ctx', c.c_void_p), ('call', HostCall), ('free', Free)]
class Plugin(c.Structure):
    _fields_ = [('version', c.c_uint32), ('call', PluginCall), ('free', Free), ('shutdown', Shutdown)]

allocations = {}
files = {
    'unrestricted': {'type': 'codex', 'disabled': False, 'access_token': 'test', 'account_id': 'unrestricted'},
    'protected': {'type': 'codex', 'disabled': False, 'access_token': 'test', 'account_id': 'protected', 'codex_quota_guard_limit': 80, 'priority': 0, 'refresh_token': 'preserve'},
    'protected-two': {'type': 'codex', 'disabled': False, 'access_token': 'test', 'account_id': 'protected-two', 'codex_quota_guard_limit': 80},
    'manual': {'type': 'codex', 'disabled': True, 'access_token': 'test', 'account_id': 'manual', 'codex_quota_guard_limit': 80},
}
usage = {'unrestricted': 100, 'protected': 79, 'protected-two': 79, 'manual': 0}
reads = []
http_failure = False
missing_window = False
save_failure = False
past_reset = False
weekly_usage = 100
missing_weekly = False

@HostCall
def callback(ctx, method, request, size, out):
    method = method.decode()
    req = json.loads(c.string_at(request, size))
    result = {}
    if method == 'host.auth.list':
        result = {'files': [{'id': k, 'auth_index': k, 'name': k+'.json', 'provider': 'codex', 'disabled': v['disabled']} for k, v in files.items()]}
    elif method == 'host.auth.get':
        result = {'json': files[req['auth_index']]}
    elif method == 'host.auth.save':
        key = req['name'].removesuffix('.json')
        if key == 'unrestricted':
            assert req['json']['disabled'] == files[key]['disabled'], 'Pacing must not impose a cutoff on unrestricted account'
        if not save_failure:
            files[key] = req['json']
    elif method == 'host.http.operation_open':
        result = {'operation_id': 'test'}
    elif method == 'host.http.cancel':
        pass
    elif method == 'host.http.do':
        assert req['method'] == 'GET', 'Guard must not send inference'
        key = req['headers']['Chatgpt-Account-Id'][0]
        reads.append(key)
        q = {'rate_limit': {'primary_window': {'limit_window_seconds': 18000, 'used_percent': usage[key], 'reset_at': int(time.time())+(-10 if past_reset else 18000)}, 'secondary_window': {'limit_window_seconds': 604800, 'used_percent': weekly_usage, 'reset_at': int(time.time())+604800}}}
        if missing_window:
            del q['rate_limit']['primary_window']
        if missing_weekly:
            del q['rate_limit']['secondary_window']
        result = {'StatusCode': 503 if http_failure else 200, 'Body': base64.b64encode(json.dumps(q).encode()).decode()}
    else:
        raise AssertionError(method)
    raw = json.dumps({'ok': False, 'error': {'code': 'disk'}} if save_failure and method == 'host.auth.save' else {'ok': True, 'result': result}).encode()
    buf = c.create_string_buffer(raw)
    address = c.addressof(buf)
    allocations[address] = buf
    out[0] = Buffer(address, len(raw))
    return 0

@Free
def free(address, size):
    allocations.pop(address, None)

with tempfile.TemporaryDirectory(prefix='cpa-guard-test-') as home:
    os.environ['USERPROFILE'] = home
    dll = c.CDLL(str(Path(__file__).resolve().parents[1]/'target/quota-guard/release/codex_window_activation.dll'))
    dll.cliproxy_plugin_init.argtypes = [c.POINTER(Host), c.POINTER(Plugin)]
    host = Host(1, None, callback, free)
    plugin = Plugin()
    assert dll.cliproxy_plugin_init(c.byref(host), c.byref(plugin)) == 0
    def call(method, req=None):
        raw = json.dumps(req or {}).encode()
        buf = Buffer()
        assert plugin.call(method.encode(), raw, len(raw), c.byref(buf)) == 0
        response = json.loads(c.string_at(buf.ptr, buf.len))
        plugin.free(buf.ptr, buf.len)
        assert response['ok'], response
        return response['result']
    def intercept(key):
        return call('request.intercept_after', {'ToFormat': 'codex', 'Model': 'gpt-6.1-sol', 'Metadata': {'selected_auth_index': key, 'selected_auth_id': key}})
    def refresh():
        call('management.handle', {'Method': 'POST', 'Path': '/plugins/codex-quota-guard/refresh'})
    reg = call('plugin.register')
    assert not reg['capabilities'].get('scheduler'), 'Native affinity must remain scheduler owner'
    assert not call('request.intercept_after', {'ToFormat':'openai', 'Metadata':{'selected_auth_id':'config-only-key'}}).get('Terminate', False), 'Unrelated config-backed providers are outside guard scope'
    assert call('request.intercept_after', {'ToFormat':'codex', 'Metadata':{}})['Terminate'], 'Missing selected Codex identity must not bypass verification'
    assert not intercept('unrestricted').get('Terminate', False)
    assert 'unrestricted' not in reads
    assert not intercept('protected').get('Terminate', False), '79 must route despite weekly exhaustion'
    usage['protected'] = 80
    assert intercept('protected')['Terminate'], 'Exactly 80 must stop'
    assert files['protected']['disabled'] and files['protected']['codex_quota_guard_paused']
    assert files['protected']['refresh_token'] == 'preserve'
    refresh()
    assert files['protected']['disabled'], 'Same capped window must remain paused'
    past_reset = True
    refresh()
    assert files['protected']['disabled'], 'Elapsed reset alone must not restore'
    past_reset = False
    usage['protected'] = 0
    refresh()
    assert not files['protected']['disabled'], 'Fresh recovered 5h window must restore account'
    assert not files['protected'].get('codex_quota_guard_paused', False)
    assert files['manual']['disabled'] and 'manual' not in reads
    assert not intercept('protected').get('Terminate', False)
    http_failure = True
    assert intercept('protected')['Terminate'], 'Quota failure must fail closed'
    assert files['protected']['disabled']
    refresh()
    assert files['protected']['disabled'], 'Refresh failure must not restore'
    http_failure = False
    missing_window = True
    refresh()
    assert files['protected']['disabled'], 'Weekly quota alone cannot restore'
    missing_window = False
    refresh()
    assert not files['protected']['disabled']
    save_failure = True
    usage['protected'] = 80
    assert intercept('protected')['Terminate'], 'Disk error must not allow capped upstream call'
    save_failure = False
    usage['protected'] = 0
    refresh()
    files['protected']['codex_quota_guard_limit'] = 'invalid'
    assert intercept('protected')['Terminate'], 'Malformed configured policy must fail closed'
    files['protected']['codex_quota_guard_limit'] = 80
    usage['protected'] = 80
    usage['protected-two'] = 80
    # Actual CPA before-selection contract leaves ToFormat empty.
    usage['protected'] = 0
    usage['protected-two'] = 0
    refresh()
    usage['protected'] = 80
    usage['protected-two'] = 80
    assert not call('request.intercept_before', {'Model': 'gpt-6.1-sol', 'ToFormat': ''}).get('Terminate', False)
    assert files['protected']['disabled'] and files['protected-two']['disabled']
    assert not files['unrestricted']['disabled'], 'Two capped accounts must leave unrestricted available'
    files['unrestricted']['disabled'] = True
    assert intercept('protected')['Terminate'] and intercept('protected-two')['Terminate'], 'Empty eligible pool cannot spill into either reserve'
    files['unrestricted']['disabled'] = False
    usage['protected-two'] = 79
    refresh()
    assert files['protected']['disabled'] and not files['protected-two']['disabled'], 'Only recovering account may restore'
    # Independent per-account reserves and weekly cap; 90 used = 10 remaining.
    files['protected']['codex_quota_guard_limit'] = 90
    files['protected']['codex_quota_guard_weekly_limit'] = 90
    files['protected-two']['codex_quota_guard_weekly_limit'] = 90
    usage['protected'] = 89
    usage['protected-two'] = 79
    weekly_usage = 89
    refresh()
    assert not intercept('protected').get('Terminate', False), '89 used must allow the 10% 5h reserve account'
    assert not intercept('protected-two').get('Terminate', False), '79 used must allow the 20% 5h reserve account'
    usage['protected'] = 90
    usage['protected-two'] = 80
    assert intercept('protected')['Terminate'] and intercept('protected-two')['Terminate'], 'Independent 5h boundaries must block'
    usage['protected'] = 0
    usage['protected-two'] = 0
    weekly_usage = 90
    refresh()
    assert files['protected']['disabled'] and files['protected-two']['disabled'], '5h reset must not lift weekly cutoff'
    assert intercept('protected')['Terminate'], 'Weekly 90 used must veto even with fresh 5h quota'
    weekly_usage = 89
    refresh()
    assert not files['protected']['disabled'] and not files['protected-two']['disabled'], 'Both safe windows may recover'
    missing_weekly = True
    assert intercept('protected')['Terminate'], 'Configured but missing weekly quota must fail closed'
    missing_weekly = False
    refresh()
    assert not files['protected']['disabled']
    files['protected']['codex_quota_guard_weekly_limit'] = 'invalid'
    assert intercept('protected')['Terminate'], 'Malformed weekly policy must fail closed'
    files['protected']['codex_quota_guard_weekly_limit'] = 90
    refresh()
    assert not intercept('unrestricted').get('Terminate', False), 'Viktor-style unrestricted account remains untouched'
    # Real DLL must publish reserve-aware weights without taking scheduler ownership.
    files['unrestricted']['codex_quota_pacing'] = True
    files['protected']['codex_quota_pacing'] = True
    files['protected-two']['codex_quota_pacing'] = True
    usage['unrestricted'] = 70
    usage['protected'] = 70
    usage['protected-two'] = 70
    weekly_usage = 50
    refresh()
    assert files['unrestricted'].get('weight', 0) > files['protected'].get('weight', 0) > files['protected-two'].get('weight', 0) > 0, 'Reserve-aware new-session weights missing'
    assert files['protected']['priority'] == 0 and files['protected']['refresh_token'] == 'preserve'
    assert files['manual']['disabled'] and 'weight' not in files['manual']
    http_failure = True
    refresh()
    assert not files['unrestricted']['disabled'] and files['unrestricted']['weight'] == 1, 'Unknown unrestricted capacity reduces assignment, not hard eligibility'
    assert files['protected']['disabled'] and files['protected-two']['disabled'], 'Pacing cannot weaken reserve fail-closed policy'
    http_failure = False
    weekly_usage = 10
    usage['unrestricted'] = usage['protected'] = usage['protected-two'] = 0
    refresh()
    assert not files['protected']['disabled'] and not files['protected-two']['disabled']
    assert all(files[k]['weight'] > 1 for k in ['unrestricted','protected','protected-two'])
    call('plugin.quiesce')
    plugin.shutdown()
    assert not allocations
    print('PASS native guard: independent 80/90 5h cutoffs, optional 90 weekly cutoff, both-window recovery, missing/malformed quota fail closed, legacy weekly-unrestricted policy, manual/unrestricted preserved, no inference, affinity preserved, shutdown')

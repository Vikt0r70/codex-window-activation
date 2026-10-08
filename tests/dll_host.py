"""Exercise the compiled DLL with the same native ABI and JSON as CPA on Windows."""
import base64
import ctypes as c
import json
import os
from pathlib import Path
import tempfile
import time

class Buffer(c.Structure):
    _fields_ = [('ptr',c.c_void_p),('len',c.c_size_t)]
HostCall=c.CFUNCTYPE(c.c_int32,c.c_void_p,c.c_char_p,c.c_void_p,c.c_size_t,c.POINTER(Buffer))
Free=c.CFUNCTYPE(None,c.c_void_p,c.c_size_t)
PluginCall=c.CFUNCTYPE(c.c_int32,c.c_char_p,c.c_void_p,c.c_size_t,c.POINTER(Buffer))
Shutdown=c.CFUNCTYPE(None)
class Host(c.Structure):
    _fields_=[('version',c.c_uint32),('ctx',c.c_void_p),('call',HostCall),('free',Free)]
class Plugin(c.Structure):
    _fields_=[('version',c.c_uint32),('call',PluginCall),('free',Free),('shutdown',Shutdown)]

allocations={}
prompts=[]
quota_reads=[]
epoch=int(time.time())
accounts=[{'name':x,'auth_index':x,'provider':'codex','disabled':x=='disabled'} for x in ['first','second','disabled','exhausted','failed']]

def quota(account):
    active=account in prompts and account!='failed'
    return {'rate_limit':{'primary_window':{'limit_window_seconds':18000,'used_percent':1 if active else 0,'reset_at':epoch+18000 if active else epoch-100},'secondary_window':{'limit_window_seconds':604800,'used_percent':100 if account=='exhausted' else 20,'reset_at':epoch+604800}}}

@HostCall
def callback(ctx,method,request,size,out):
    req=json.loads(c.string_at(request,size)); method=method.decode()
    if method=='host.auth.list':result={'files':accounts}
    elif method=='host.auth.get':result={'json':{'access_token':req['auth_index'],'account_id':req['auth_index'],'disabled':False}}
    elif method=='host.http.operation_open':result={'operation_id':'test-op'}
    elif method=='host.http.cancel':result={}
    elif method=='host.http.do':
        account=req['headers']['Chatgpt-Account-Id'][0]
        assert req['headers']['Authorization']==['Bearer '+account]
        if req['method']=='GET':
            quota_reads.append(account);body=json.dumps(quota(account)).encode()
        else:
            assert account not in ['disabled','exhausted']
            payload=json.loads(bytes(req['body']))
            assert payload['model']=='gpt-6.1-sol'
            assert payload['stream'] and payload['store'] is False
            prompts.append(account)
            body=b'data: {"type":"response.completed","response":{"status":"completed"}}\n\n'
        result={'StatusCode':429 if account=='failed' and req['method']=='POST' else 200,'Body':base64.b64encode(body).decode()}
    else:raise AssertionError(method)
    raw=json.dumps({'ok':True,'result':result}).encode();buf=c.create_string_buffer(raw)
    address=c.addressof(buf);allocations[address]=buf;out[0]=Buffer(address,len(raw));return 0

@Free
def free(address,size):allocations.pop(address,None)

with tempfile.TemporaryDirectory(prefix='cpa-plugin-test-') as home:
    os.environ['USERPROFILE']=home
    Path(home,'.cli-proxy-api').mkdir()
    dll=c.CDLL(str(Path(__file__).resolve().parents[1]/'target/release/codex_window_activation.dll'))
    dll.cliproxy_plugin_init.argtypes=[c.POINTER(Host),c.POINTER(Plugin)]
    host=Host(1,None,callback,free);plugin=Plugin()
    assert dll.cliproxy_plugin_init(c.byref(host),c.byref(plugin))==0
    def call(method):
        raw=b'{}';buf=Buffer()
        assert plugin.call(method.encode(),raw,len(raw),c.byref(buf))==0
        result=json.loads(c.string_at(buf.ptr,buf.len));plugin.free(buf.ptr,buf.len)
        assert result['ok'],result
        return result['result']
    call('plugin.register')
    deadline=time.time()+10
    while time.time()<deadline:
        report=call('management.handle')
        status=json.loads(bytes(report['Body']))
        if status and status.get('accounts'):break
        time.sleep(.05)
    assert prompts==['first','second','failed'],prompts
    assert quota_reads.count('first')==2 and quota_reads.count('second')==2
    assert 'disabled' not in quota_reads
    call('plugin.quiesce')
    state=Path(home,'.cli-proxy-api/codex-window-activation-state.json')
    assert state.exists() and 'access_token' not in state.read_text()
    persisted=json.loads(state.read_text())
    assert persisted['accounts']['failed']['completed']==[]
    assert persisted['accounts']['failed']['retry_after']>int(time.time())
    assert list(persisted['accounts']['failed']['attempts'].values())==[1]
    call('plugin.reconfigure')
    deadline=time.time()+10
    while time.time()<deadline and quota_reads.count('first')<3:time.sleep(.05)
    call('plugin.quiesce')
    assert prompts==['first','second','failed'],'Restart duplicated activation or ignored failure backoff'
    persisted=json.loads(state.read_text())
    failed=persisted['accounts']['failed'];failed['retry_after']=0
    failed['attempts']={str(epoch-100):3}
    state.write_text(json.dumps(persisted))
    call('plugin.reconfigure')
    deadline=time.time()+10
    while time.time()<deadline and quota_reads.count('failed')<3:time.sleep(.05)
    call('plugin.quiesce')
    assert prompts==['first','second','failed'],'Exceeded three attempts per reset'
    plugin.shutdown()
    assert not allocations,'Host response buffers leaked'
    print('PASS: native ABI, exact-account credentials, two successful activations, skipped disabled/exhausted, verification, persistent dedup/reconfigure, failure backoff, three-attempt cap, buffers freed, shutdown')

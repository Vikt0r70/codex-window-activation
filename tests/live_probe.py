"""One real, small inference through the compiled DLL, using an isolated state home.

No production reset records, quota settings or credentials are modified.
"""
import base64
import ctypes as c
import json
import os
from pathlib import Path
import tempfile
import time
import urllib.request

class Buffer(c.Structure):
    _fields_=[('ptr',c.c_void_p),('len',c.c_size_t)]
HostCall=c.CFUNCTYPE(c.c_int32,c.c_void_p,c.c_char_p,c.c_void_p,c.c_size_t,c.POINTER(Buffer))
Free=c.CFUNCTYPE(None,c.c_void_p,c.c_size_t)
PluginCall=c.CFUNCTYPE(c.c_int32,c.c_char_p,c.c_void_p,c.c_size_t,c.POINTER(Buffer))
Shutdown=c.CFUNCTYPE(None)
class Host(c.Structure):
    _fields_=[('version',c.c_uint32),('ctx',c.c_void_p),('call',HostCall),('free',Free)]
class Plugin(c.Structure):
    _fields_=[('version',c.c_uint32),('call',PluginCall),('free',Free),('shutdown',Shutdown)]

auth_file=Path(os.environ['CPA_LIVE_AUTH_FILE'])
auth=json.loads(auth_file.read_text())
assert auth.get('disabled') is False
allocations={}
requests=[]
results=[]

@HostCall
def callback(ctx,method,request,size,out):
    req=json.loads(c.string_at(request,size));method=method.decode()
    try:
        if method=='host.auth.list':result={'files':[{'provider':'codex','auth_index':'live-test-account','name':'Live probe account','disabled':False}]}
        elif method=='host.auth.get':result={'json':auth}
        elif method=='host.http.operation_open':result={'operation_id':'live-op'}
        elif method=='host.http.cancel':result={}
        elif method=='host.http.do':
            headers={k:v[0] for k,v in req['headers'].items()}
            body=bytes(req.get('body',[])) or None
            request=urllib.request.Request(req['url'],headers=headers,data=body,method=req['method'])
            with urllib.request.urlopen(request,timeout=40) as response:
                raw=response.read();status=response.status
            requests.append(req['method'])
            if req['method']=='GET':
                q=json.loads(raw)
                windows=q['rate_limit']
                results.append({key:{k:v for k,v in value.items() if k in ['used_percent','reset_at','limit_window_seconds']} for key,value in windows.items() if isinstance(value,dict)})
                if requests==['GET']:
                    # Test-only clock boundary: induce exactly one due 5h reset.
                    assert windows['primary_window']['used_percent']<100
                    assert windows['secondary_window']['used_percent']<100
                    windows['primary_window']['reset_at']=int(time.time())-100
                    raw=json.dumps(q).encode()
            else:
                completed=[json.loads(line[5:].strip()) for line in raw.decode().splitlines() if line.startswith('data:') and 'response.completed' in line]
                assert completed,'Provider did not complete the tiny prompt'
                r=completed[-1]['response']
                results.append({'inference':'completed','model':r.get('model'),'usage':r.get('usage')})
            result={'StatusCode':status,'Body':base64.b64encode(raw).decode()}
        else:raise ValueError('Unsupported callback')
        envelope={'ok':True,'result':result}
    except Exception as error:
        envelope={'ok':False,'error':{'code':'test_http_failure','http_status':getattr(error,'code',0)}}
    raw=json.dumps(envelope).encode();buf=c.create_string_buffer(raw);address=c.addressof(buf)
    allocations[address]=buf;out[0]=Buffer(address,len(raw));return 0

@Free
def free(address,size):allocations.pop(address,None)

with tempfile.TemporaryDirectory(prefix='cpa-plugin-live-') as home:
    os.environ['USERPROFILE']=home;Path(home,'.cli-proxy-api').mkdir()
    dll=c.CDLL(str(Path(__file__).resolve().parents[1]/'target/release/codex_window_activation.dll'))
    dll.cliproxy_plugin_init.argtypes=[c.POINTER(Host),c.POINTER(Plugin)]
    host=Host(1,None,callback,free);plugin=Plugin();assert dll.cliproxy_plugin_init(c.byref(host),c.byref(plugin))==0
    def call(method):
        buf=Buffer();raw=b'{}';assert plugin.call(method.encode(),raw,len(raw),c.byref(buf))==0
        v=json.loads(c.string_at(buf.ptr,buf.len));plugin.free(buf.ptr,buf.len);assert v['ok'],v
        return v['result']
    call('plugin.register')
    deadline=time.time()+110;status=None
    while time.time()<deadline:
        status=json.loads(bytes(call('management.handle')['Body']))
        if status and status.get('accounts'):break
        time.sleep(.1)
    call('plugin.quiesce');plugin.shutdown()
    print(json.dumps({'requests':requests,'results':results,'status':status},indent=2))
    assert requests==['GET','POST','GET'],requests
    assert status['accounts'][0]['status']=='activation completed; quota refreshed',status
    print('PASS: real Codex completion on the selected account, followed by live quota verification; production state untouched')

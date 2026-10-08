use serde_json::{json, Value};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    fs,
    path::PathBuf,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Condvar, Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const ID: &str = "codex-window-activation";
const USER_AGENT: &str =
    "codex-tui/0.154.0 (Mac OS 26.5.2; arm64) iTerm.app/3.6.11 (codex-tui; 0.154.0)";

#[derive(Debug, PartialEq)]
pub enum Decision {
    Wait,
    Activate,
    Exhausted,
}
#[derive(Debug)]
pub struct Window {
    pub seconds: i64,
    pub used: f64,
    pub reset: i64,
}

pub fn windows(q: &Value) -> Result<Vec<Window>, String> {
    let mut result = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        let w = &q["rate_limit"][key];
        let Some(seconds) = w["limit_window_seconds"].as_i64() else {
            continue;
        };
        if ![18000, 604800].contains(&seconds) {
            continue;
        }
        let used = w["used_percent"].as_f64().ok_or("Missing usage")?;
        let reset = w["reset_at"].as_i64().ok_or("Missing reset deadline")?;
        if !(0.0..=100.0).contains(&used) || reset <= 0 {
            return Err("Invalid quota".into());
        }
        result.push(Window {
            seconds,
            used,
            reset,
        });
    }
    if result.is_empty() {
        return Err("No supported quota windows".into());
    }
    Ok(result)
}

fn due_deadlines(w: &[Window], now: i64, state: &Value) -> Vec<i64> {
    w.iter()
        .filter_map(|w| {
            let previous = state["next"][w.seconds.to_string()]
                .as_i64()
                .unwrap_or(w.reset);
            let deadline = previous.min(w.reset);
            let completed = state["completed"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v.as_i64() == Some(deadline)));
            // Positive usage in a future window means a real request already started it.
            if now >= deadline + 30 && !completed && !(w.reset > now && w.used > 0.0) {
                Some(deadline)
            } else {
                None
            }
        })
        .collect()
}

pub fn decision(w: &[Window], now: i64, state: &Value) -> Decision {
    if state["retry_after"].as_i64().unwrap_or(0) > now {
        return Decision::Wait;
    }
    if w.iter().any(|w| w.used >= 100.0 && w.reset > now) {
        return Decision::Exhausted;
    }
    if due_deadlines(w, now, state).is_empty() {
        Decision::Wait
    } else {
        Decision::Activate
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn state_path() -> PathBuf {
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default())
        .join(".cli-proxy-api")
        .join("codex-window-activation-state.json")
}

#[repr(C)]
pub struct Buffer {
    pub ptr: *mut u8,
    pub len: usize,
}
type HostCall =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const u8, usize, *mut Buffer) -> i32;
type Free = unsafe extern "C" fn(*mut c_void, usize);
#[repr(C)]
pub struct HostApi {
    abi_version: u32,
    host_ctx: *mut c_void,
    call: Option<HostCall>,
    free_buffer: Option<Free>,
}
#[repr(C)]
pub struct PluginApi {
    abi_version: u32,
    call: Option<unsafe extern "C" fn(*const c_char, *const u8, usize, *mut Buffer) -> i32>,
    free_buffer: Option<Free>,
    shutdown: Option<unsafe extern "C" fn()>,
}
#[derive(Clone, Copy)]
struct Host {
    context: usize,
    call: HostCall,
    free: Free,
}
static HOST: OnceLock<Host> = OnceLock::new();
static WORKER: Mutex<Option<thread::JoinHandle<()>>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);
static WAKE: Condvar = Condvar::new();
static WAIT: Mutex<()> = Mutex::new(());
static REPORT: Mutex<Value> = Mutex::new(Value::Null);
static ACTIVE_OPERATION: Mutex<Option<String>> = Mutex::new(None);

fn host(method: &str, request: Value) -> Result<Value, String> {
    let h = HOST.get().ok_or("Host unavailable")?;
    let method = CString::new(method).map_err(|_| "Invalid method")?;
    let bytes = serde_json::to_vec(&request).map_err(|_| "Invalid request")?;
    let mut response = Buffer {
        ptr: ptr::null_mut(),
        len: 0,
    };
    let code = unsafe {
        (h.call)(
            h.context as *mut c_void,
            method.as_ptr(),
            bytes.as_ptr(),
            bytes.len(),
            &mut response,
        )
    };
    let raw = if response.ptr.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(response.ptr, response.len) }.to_vec()
    };
    if !response.ptr.is_null() {
        unsafe { (h.free)(response.ptr as *mut c_void, response.len) }
    }
    let envelope: Value = serde_json::from_slice(&raw).map_err(|_| "Invalid host response")?;
    if code != 0 || envelope["ok"] != true {
        // Host error text may contain upstream body or credential material; do not persist it.
        return Err(format!(
            "Host callback failed ({})",
            envelope["error"]["http_status"].as_i64().unwrap_or(0)
        ));
    }
    Ok(envelope["result"].clone())
}

fn decode_body(value: &Value) -> Result<Vec<u8>, String> {
    if let Some(a) = value.as_array() {
        return a
            .iter()
            .map(|v| {
                v.as_u64()
                    .filter(|v| *v <= 255)
                    .map(|v| v as u8)
                    .ok_or("Invalid body byte".into())
            })
            .collect();
    }
    let s = value.as_str().ok_or("Missing response body")?;
    let mut out = Vec::new();
    let mut accumulator = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let n = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err("Invalid base64".into()),
        };
        accumulator = (accumulator << 6) | n as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
            accumulator &= (1u32 << bits) - 1;
        }
    }
    Ok(out)
}

fn http(method: &str, url: &str, auth: &Value, body: Option<Value>) -> Result<Vec<u8>, String> {
    if STOP.load(Ordering::SeqCst) {
        return Err("Stopping".into());
    }
    let token = auth["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Missing access token")?;
    let account = auth["account_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Missing account id")?;
    let op = host("host.http.operation_open", json!({}))?["operation_id"]
        .as_str()
        .ok_or("Missing operation id")?
        .to_owned();
    *ACTIVE_OPERATION.lock().unwrap() = Some(op.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let cancel_op = op.clone();
    let watchdog = thread::spawn(move || {
        if rx.recv_timeout(Duration::from_secs(45)).is_err() {
            let _ = host("host.http.cancel", json!({"operation_id":cancel_op}));
        }
    });
    let bytes = body
        .map(|v| serde_json::to_vec(&v).unwrap())
        .unwrap_or_default();
    let result = host(
        "host.http.do",
        json!({"operation_id":op,"method":method,"url":url,"headers":{
        "Authorization":[format!("Bearer {token}")],"Chatgpt-Account-Id":[account],"Content-Type":["application/json"],
        "User-Agent":[USER_AGENT],"Originator":["codex-tui"],"Accept":[if method=="POST"{"text/event-stream"}else{"application/json"}]
    },"body":bytes}),
    );
    let _ = tx.send(());
    let _ = watchdog.join();
    *ACTIVE_OPERATION.lock().unwrap() = None;
    let response = result?;
    let status = response["StatusCode"]
        .as_i64()
        .ok_or("Missing HTTP status")?;
    if !(200..300).contains(&status) {
        return Err(format!("Upstream HTTP {status}"));
    }
    decode_body(&response["Body"])
}

fn quota(auth: &Value) -> Result<Vec<Window>, String> {
    let bytes = http(
        "GET",
        "https://chatgpt.com/backend-api/wham/usage",
        auth,
        None,
    )?;
    windows(&serde_json::from_slice(&bytes).map_err(|_| "Invalid usage response")?)
}
fn activate(auth: &Value) -> Result<(), String> {
    let bytes = http(
        "POST",
        "https://chatgpt.com/backend-api/codex/responses",
        auth,
        Some(json!({
            "model":"gpt-6.1-sol","instructions":"Reply with one word only.","input":[{"role":"user","content":[{"type":"input_text","text":"Say OK."}]}],
            "reasoning":{"effort":"low"},"text":{"verbosity":"low"},"stream":true,"store":false,"tools":[]
        })),
    )?;
    for line in String::from_utf8_lossy(&bytes).lines() {
        if let Some(raw) = line.strip_prefix("data:") {
            if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
                if v["type"] == "response.completed" {
                    return Ok(());
                }
            }
        }
    }
    Err("No successful inference completion".into())
}

fn save(state: &Value) -> Result<(), String> {
    let path = state_path();
    let tmp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(state).map_err(|_| "Cannot encode state")?;
    fs::write(&tmp, bytes).map_err(|_| "Cannot persist state")?;
    // Windows ReplaceFile is unnecessary: rename replaces files on this Rust target.
    fs::rename(tmp, path).map_err(|_| "Cannot commit state".to_owned())
}

fn poll(state: &mut Value) -> Result<(), String> {
    let files = host("host.auth.list", json!({}))?;
    let files = files["files"].as_array().ok_or("Missing accounts list")?;
    let mut accounts = Vec::new();
    for file in files {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        if file["provider"] != "codex" && file["type"] != "codex" {
            continue;
        }
        let id = file["auth_index"].as_str().ok_or("Missing auth index")?;
        let name = file["name"].as_str().unwrap_or(id);
        if file["disabled"] == true {
            accounts.push(json!({"account":name,"status":"disabled; skipped"}));
            continue;
        }
        if !state["accounts"][id].is_object() {
            state["accounts"][id] = json!({"next":{},"completed":[]});
        }
        let result = (|| -> Result<Value, String> {
            let auth = host("host.auth.get", json!({"auth_index":id}))?["json"].clone();
            if auth["disabled"] == true {
                return Ok(json!({"account":name,"status":"disabled; skipped"}));
            }
            let current = quota(&auth)?;
            let t = now();
            let entry = &mut state["accounts"][id];
            let action = decision(&current, t, entry);
            if action == Decision::Activate {
                let deadlines = due_deadlines(&current, t, entry);
                let attempt_key = deadlines.iter().min().unwrap().to_string();
                let attempts = entry["attempts"][&attempt_key].as_u64().unwrap_or(0);
                if attempts >= 3 {
                    return Ok(
                        json!({"account":name,"status":"three activation attempts failed; waiting for real traffic or a new reset"}),
                    );
                }
                if !entry["attempts"].is_object() {
                    entry["attempts"] = json!({});
                }
                entry["attempts"][attempt_key] = json!(attempts + 1);
                // Persist the backoff before the upstream request, including across crashes.
                entry["last_attempt"] = json!(t);
                entry["retry_after"] = json!(t + 900);
                save(state)?;
                activate(&auth)?;
                let entry = &mut state["accounts"][id];
                entry["last_success"] = json!(now());
                entry["retry_after"] = json!(0);
                let done = entry["completed"].as_array_mut().unwrap();
                for d in deadlines {
                    if !done.contains(&json!(d)) {
                        done.push(json!(d));
                    }
                }
                if done.len() > 32 {
                    done.drain(..done.len() - 32);
                }
                // Success prevents repeated spending even if subsequent quota refresh fails.
                save(state)?;
                let after = quota(&auth)?;
                for w in &after {
                    state["accounts"][id]["next"][w.seconds.to_string()] = json!(w.reset);
                }
                state["accounts"][id]["last_verification"] = json!(now());
                Ok(
                    json!({"account":name,"status":"activation completed; quota refreshed","windows":after.iter().map(|w|json!({"seconds":w.seconds,"used_percent":w.used,"reset_at":w.reset})).collect::<Vec<_>>()}),
                )
            } else {
                for w in &current {
                    let key = w.seconds.to_string();
                    let old = entry["next"][&key].as_i64().unwrap_or(w.reset);
                    // Keep an expired remembered deadline until activated, unless real traffic started the new window.
                    let done = entry["completed"]
                        .as_array()
                        .is_some_and(|a| a.contains(&json!(old)));
                    if old > t
                        || (w.reset > t && w.used > 0.0)
                        || done
                        || entry["next"][&key].is_null()
                    {
                        entry["next"][key] = json!(w.reset);
                    }
                }
                Ok(
                    json!({"account":name,"status":if action==Decision::Exhausted{"exhausted; no prompt"}else{"watching reset deadlines"},"windows":current.iter().map(|w|json!({"seconds":w.seconds,"used_percent":w.used,"reset_at":w.reset})).collect::<Vec<_>>()}),
                )
            }
        })();
        accounts.push(result.unwrap_or_else(|e| json!({"account":name,"status":e})));
    }
    state["checked_at"] = json!(now());
    save(state)?;
    *REPORT.lock().unwrap() = json!({"running":true,"checked_at":now(),"accounts":accounts,"state_file":state_path(),"poll_seconds":60});
    Ok(())
}

fn start() -> Result<(), String> {
    stop();
    STOP.store(false, Ordering::SeqCst);
    let state = if state_path().exists() {
        serde_json::from_slice::<Value>(&fs::read(state_path()).map_err(|_| "Cannot read state")?)
            .map_err(|_| "Invalid persistent state; refusing duplicate activation")?
    } else {
        json!({"accounts":{}})
    };
    if !state["accounts"].is_object() {
        return Err("Invalid state format".into());
    }
    *WORKER.lock().unwrap() = Some(thread::spawn(move || {
        let mut state = state;
        while !STOP.load(Ordering::SeqCst) {
            if let Err(e) = poll(&mut state) {
                *REPORT.lock().unwrap() = json!({"running":true,"error":e,"checked_at":now()});
            }
            let guard = WAIT.lock().unwrap();
            let _ = WAKE.wait_timeout_while(guard, Duration::from_secs(60), |_| {
                !STOP.load(Ordering::SeqCst)
            });
        }
    }));
    Ok(())
}
fn stop() {
    STOP.store(true, Ordering::SeqCst);
    WAKE.notify_all();
    if let Some(op) = ACTIVE_OPERATION.lock().unwrap().clone() {
        let _ = host("host.http.cancel", json!({"operation_id":op}));
    }
    if let Some(w) = WORKER.lock().unwrap().take() {
        let _ = w.join();
    }
}

fn handle(method: &str, _req: &Value) -> Result<Value, String> {
    match method {
        "plugin.register" | "plugin.reconfigure" => {
            start()?;
            Ok(
                json!({"schema_version":6,"metadata":{"Name":ID,"Version":"0.1.0","Author":"Local custom plugin","GitHubRepository":"https://github.com/Vikt0r70/codex-window-activation","ConfigFields":[]},"capabilities":{"management_api":true}}),
            )
        }
        "plugin.quiesce" | "plugin.shutdown" => {
            stop();
            Ok(json!({}))
        }
        "management.register" => Ok(
            json!({"routes":[{"Method":"GET","Path":"/plugins/codex-window-activation/status","Description":"Silent per-account reset activation and last quota readings"}]}),
        ),
        "management.handle" => {
            let body =
                serde_json::to_vec(&*REPORT.lock().unwrap()).map_err(|_| "Invalid status")?;
            Ok(
                json!({"StatusCode":200,"Headers":{"Content-Type":["application/json"]},"Body":body}),
            )
        }
        _ => Err("Unknown method".into()),
    }
}

#[no_mangle]
/// # Safety
/// The host must supply valid ABI tables and keep callbacks alive until shutdown.
pub unsafe extern "C" fn cliproxy_plugin_init(h: *const HostApi, p: *mut PluginApi) -> i32 {
    if h.is_null() || p.is_null() || (*h).abi_version != 1 {
        return 1;
    }
    let (Some(call), Some(free)) = ((*h).call, (*h).free_buffer) else {
        return 1;
    };
    let _ = HOST.set(Host {
        context: (*h).host_ctx as usize,
        call,
        free,
    });
    *p = PluginApi {
        abi_version: 1,
        call: Some(plugin_call),
        free_buffer: Some(plugin_free),
        shutdown: Some(plugin_shutdown),
    };
    0
}
unsafe extern "C" fn plugin_call(
    method: *const c_char,
    request: *const u8,
    len: usize,
    response: *mut Buffer,
) -> i32 {
    if response.is_null() {
        return 1;
    }
    *response = Buffer {
        ptr: ptr::null_mut(),
        len: 0,
    };
    let result = std::panic::catch_unwind(|| {
        if method.is_null() {
            return Err("Missing method".to_owned());
        }
        let m = CStr::from_ptr(method)
            .to_str()
            .map_err(|_| "Invalid method")?;
        let req = if len == 0 {
            json!({})
        } else {
            if request.is_null() {
                return Err("Missing request".into());
            }
            serde_json::from_slice(std::slice::from_raw_parts(request, len))
                .map_err(|_| "Invalid JSON")?
        };
        handle(m, &req)
    });
    let envelope = match result {
        Ok(Ok(v)) => json!({"ok":true,"result":v}),
        Ok(Err(e)) => json!({"ok":false,"error":{"code":"plugin_error","message":e}}),
        Err(_) => json!({"ok":false,"error":{"code":"panic","message":"Plugin operation failed"}}),
    };
    let bytes = serde_json::to_vec(&envelope).unwrap().into_boxed_slice();
    (*response).len = bytes.len();
    (*response).ptr = Box::into_raw(bytes) as *mut u8;
    0
}
unsafe extern "C" fn plugin_free(p: *mut c_void, len: usize) {
    if !p.is_null() {
        drop(Box::from_raw(ptr::slice_from_raw_parts_mut(
            p as *mut u8,
            len,
        )));
    }
}
unsafe extern "C" fn plugin_shutdown() {
    stop();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_host_body_decodes() {
        assert_eq!(
            decode_body(&json!("eyJvayI6dHJ1ZX0=")).unwrap(),
            b"{\"ok\":true}"
        );
    }
    #[test]
    fn old_due_deadline_survives_upstream_advance_without_usage() {
        let w=windows(&json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"used_percent":0,"reset_at":20000}}})).unwrap();
        assert_eq!(
            decision(&w, 1100, &json!({"next":{"18000":1000}})),
            Decision::Activate
        );
    }
}

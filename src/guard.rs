use super::*;

static GATE: Mutex<()> = Mutex::new(());

pub fn capped(quota: &Value, limit: f64, _now: i64) -> Result<bool, String> {
    let mut found = None;
    for key in ["primary_window", "secondary_window"] {
        let w = &quota["rate_limit"][key];
        if w["limit_window_seconds"].as_i64() != Some(18000) {
            continue;
        }
        if found.is_some() {
            return Err("Ambiguous five-hour quota".into());
        }
        let used = w["used_percent"]
            .as_f64()
            .ok_or("Missing five-hour usage")?;
        if !(0.0..=100.0).contains(&used) || w["reset_at"].as_i64().unwrap_or(0) <= 0 {
            return Err("Invalid five-hour quota".into());
        }
        found = Some(used >= limit);
    }
    found.ok_or("Missing five-hour quota".into())
}

fn check(file: &Value) -> Result<Value, String> {
    let index = file["auth_index"].as_str().ok_or("Missing auth index")?;
    let name = file["name"].as_str().ok_or("Missing auth filename")?;
    let mut auth = host("host.auth.get", json!({"auth_index":index}))?["json"].clone();
    if auth.get("codex_quota_guard_limit").is_none() {
        return Ok(json!({"auth_index":index,"protected":false,"blocked":false}));
    }
    let limit = auth["codex_quota_guard_limit"].as_f64().unwrap_or(f64::NAN);
    let owned = auth["codex_quota_guard_paused"] == true;
    if (auth["disabled"] == true || file["disabled"] == true) && !owned {
        return Ok(
            json!({"auth_index":index,"protected":true,"blocked":true,"status":"manually disabled; unchanged"}),
        );
    }
    let reading = (|| {
        if !(0.0..100.0).contains(&limit) {
            return Err("Invalid guard threshold".into());
        }
        // The host global HTTP transport does not honor per-auth proxies. Refuse
        // that independently exposed configuration rather than send direct.
        if auth["proxy_url"].as_str().is_some_and(|s| !s.is_empty())
            || auth["proxy-url"].as_str().is_some_and(|s| !s.is_empty())
        {
            return Err("Per-account proxy unsupported; blocked".into());
        }
        let raw = http(
            "GET",
            "https://chatgpt.com/backend-api/wham/usage",
            &auth,
            None,
        )?;
        let q: Value = serde_json::from_slice(&raw).map_err(|_| "Invalid quota response")?;
        let block = capped(&q, limit, now())?;
        let w = ["primary_window", "secondary_window"]
            .into_iter()
            .map(|k| &q["rate_limit"][k])
            .find(|w| w["limit_window_seconds"] == 18000)
            .unwrap();
        Ok((block, w["used_percent"].clone(), w["reset_at"].clone()))
    })();
    let (blocked, used, reset, status) = match reading {
        Ok((b, u, r)) => (
            b,
            u,
            r,
            if b {
                "five-hour cutoff reached".to_owned()
            } else {
                "live quota below cutoff".to_owned()
            },
        ),
        Err(e) => (true, Value::Null, Value::Null, e),
    };
    if blocked && (!owned || auth["disabled"] != true) {
        // Quota HTTP can overlap the core's OAuth refresh. Re-read immediately
        // before saving so a refreshed token is not replaced by our old copy.
        auth = host("host.auth.get", json!({"auth_index":index}))?["json"].clone();
        auth["disabled"] = json!(true);
        auth["codex_quota_guard_paused"] = json!(true);
        host("host.auth.save", json!({"name":name,"json":auth}))?;
    } else if !blocked && owned {
        auth = host("host.auth.get", json!({"auth_index":index}))?["json"].clone();
        if auth["codex_quota_guard_paused"] != true {
            return Err("Pause ownership changed; not restored".into());
        }
        auth["disabled"] = json!(false);
        auth.as_object_mut()
            .ok_or("Invalid auth object")?
            .remove("codex_quota_guard_paused");
        host("host.auth.save", json!({"name":name,"json":auth}))?;
    }
    Ok(
        json!({"auth_index":index,"protected":true,"blocked":blocked,"limit_used_percent":limit,
        "five_hour_used_percent":used,"reset_at":reset,"status":status,"checked_at":now()}),
    )
}

fn poll() -> Result<(), String> {
    let _gate = GATE.lock().map_err(|_| "Guard lock failed")?;
    let roster = host("host.auth.list", json!({}))?;
    let files = roster["files"].as_array().ok_or("Missing auth list")?;
    let mut accounts = Vec::new();
    for f in files {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        if f["provider"] != "codex" && f["type"] != "codex" {
            continue;
        }
        let result = check(f)
            .unwrap_or_else(|e| json!({"auth_index":f["auth_index"],"blocked":true,"error":e}));
        accounts.push(result);
    }
    *REPORT.lock().map_err(|_| "Report lock failed")? =
        json!({"running":true,"accounts":accounts,"checked_at":now(),"poll_seconds":30});
    Ok(())
}

pub(crate) fn terminal(message: &str) -> Value {
    let body = serde_json::to_vec(&json!({"error":{"code":"codex_quota_guard","message":message}}))
        .unwrap();
    json!({"Terminate":true,"StatusCode":429,"ResponseHeaders":{"Content-Type":["application/json"]},"ResponseBody":body})
}

fn intercept(req: &Value) -> Value {
    // CPA supplies the selected executor's format after selection. Config-backed
    // non-Codex keys need not appear in host.auth.list, and are outside this guard.
    if req["ToFormat"] != "codex" {
        return json!({});
    }
    let result = (|| -> Result<Value, String> {
        let _gate = GATE.lock().map_err(|_| "Guard lock failed")?;
        let meta = &req["Metadata"];
        let index = meta["selected_auth_index"].as_str();
        let id = meta["selected_auth_id"].as_str();
        // After selection, Codex must have an exact credential identity.
        // Without it, the protected account cannot be verified safely.
        if index.is_none() && id.is_none() {
            return Err("Missing selected Codex credential identity".into());
        }
        let roster = host("host.auth.list", json!({}))?;
        let file = roster["files"]
            .as_array()
            .ok_or("Missing auth list")?
            .iter()
            .find(|f| {
                index.is_some_and(|i| f["auth_index"] == i) || id.is_some_and(|i| f["id"] == i)
            });
        let f = file.ok_or("Selected credential missing; refused")?;
        if f["provider"] != "codex" && f["type"] != "codex" {
            return Ok(json!({}));
        }
        let report = check(f)?;
        if report["blocked"] == true {
            return Ok(terminal("Selected account is paused at its quota cutoff or quota cannot be verified. Retry to use another eligible account."));
        }
        Ok(json!({}))
    })();
    result.unwrap_or_else(|_| {
        terminal("Quota guard could not verify selected credential; no upstream request sent.")
    })
}

pub fn handle(method: &str, req: &Value) -> Result<Value, String> {
    match method {
        "plugin.register" | "plugin.reconfigure" => {
            super::stop();
            STOP.store(false, Ordering::SeqCst);
            *WORKER.lock().unwrap() = Some(thread::spawn(|| {
                while !STOP.load(Ordering::SeqCst) {
                    let result = std::panic::catch_unwind(poll);
                    if !matches!(result, Ok(Ok(()))) {
                        *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = json!({"running":true,"error":"Quota poll failed; protected requests independently checked","checked_at":now()});
                    }
                    let lock = WAIT.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = WAKE.wait_timeout_while(lock, Duration::from_secs(30), |_| {
                        !STOP.load(Ordering::SeqCst)
                    });
                }
            }));
            Ok(
                json!({"schema_version":6,"metadata":{"Name":ID,"Version":"0.2.0","Author":"Local quota policy","GitHubRepository":"https://github.com/Vikt0r70/codex-window-activation","ConfigFields":[]},
                "capabilities":{"management_api":true,"request_interceptor":true}}),
            )
        }
        "plugin.quiesce" | "plugin.shutdown" => {
            stop();
            Ok(json!({}))
        }
        "request.intercept_before" => {
            // CPA has no selected upstream/provider before selection. Refresh
            // only the Codex roster, without guessing a provider from model names.
            // A failed poll must not block unrelated/unrestricted providers;
            // the selected-credential gate verifies protected calls separately.
            let _ = poll();
            Ok(json!({}))
        }
        "request.intercept_after" => Ok(intercept(req)),
        "management.register" => Ok(json!({"routes":[
            {"Method":"GET","Path":"/plugins/codex-quota-guard/status"},
            {"Method":"POST","Path":"/plugins/codex-quota-guard/refresh"}
        ]})),
        "management.handle" => {
            if req["Method"] == "POST"
                && req["Path"]
                    .as_str()
                    .is_some_and(|s| s.ends_with("/refresh"))
            {
                poll()?;
            }
            let body =
                serde_json::to_vec(&*REPORT.lock().unwrap()).map_err(|_| "Invalid report")?;
            Ok(
                json!({"StatusCode":200,"Headers":{"Content-Type":["application/json"]},"Body":body}),
            )
        }
        _ => Err("Unknown method".into()),
    }
}

pub fn stop() {
    super::stop();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reading(used: f64, reset: i64) -> Value {
        json!({"rate_limit":{
            "secondary_window":{"limit_window_seconds":18000,"used_percent":used,"reset_at":reset},
            "primary_window":{"limit_window_seconds":604800,"used_percent":100,"reset_at":999999}
        }})
    }
    #[test]
    fn exactly_eighty_stops_and_weekly_is_ignored() {
        assert!(!capped(&reading(79.0, 2000), 80.0, 1000).unwrap());
        assert!(capped(&reading(80.0, 2000), 80.0, 1000).unwrap());
        assert!(capped(&reading(100.0, 2000), 80.0, 1000).unwrap());
    }
    #[test]
    fn elapsed_reset_does_not_override_high_usage() {
        assert!(capped(&reading(80.0, 900), 80.0, 1000).unwrap());
        assert!(!capped(&reading(0.0, 2000), 80.0, 1000).unwrap());
    }
    #[test]
    fn missing_invalid_or_ambiguous_five_hour_fails_closed() {
        assert!(capped(&json!({}), 80.0, 1000).is_err());
        assert!(capped(&reading(101.0, 2000), 80.0, 1000).is_err());
        assert!(capped(&reading(0.0, 0), 80.0, 1000).is_err());
        let mut q = reading(0.0, 2000);
        q["rate_limit"]["primary_window"] = q["rate_limit"]["secondary_window"].clone();
        assert!(capped(&q, 80.0, 1000).is_err());
    }
}

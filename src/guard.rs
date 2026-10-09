use super::*;

static GATE: Mutex<()> = Mutex::new(());

fn assignment_weight(windows: [(f64, f64, i64, i64); 2], now: i64, rate: Option<f64>) -> i64 {
    let mut score: f64 = 1.0;
    for (used, limit, reset, duration) in windows {
        if !(0.0..=100.0).contains(&used) || limit <= 0.0 || limit > 100.0 || reset <= now {
            return 1;
        }
        let spendable = (limit - used).max(0.0);
        if spendable == 0.0 {
            return 1;
        }
        let capacity = spendable / limit;
        let time_fraction = ((reset - now) as f64 / duration as f64).clamp(0.0, 1.0);
        // Both absolute runway and pacing matter. Near-reset spare capacity is
        // useful, but a tiny weekly balance cannot win solely on reset urgency.
        score = score.min(capacity * (1.0 + capacity - time_fraction));
    }
    if let Some(rate) = rate.filter(|r| r.is_finite() && *r > 0.0) {
        let runway = (windows[0].1 - windows[0].0).max(0.0) / rate;
        // Reduce new work when observed consumption would use the short-window
        // allowance within 15 minutes. Never zero a healthy warm binding.
        score *= (runway / 900.0).clamp(0.05, 1.0);
    }
    (score * 1000.0).round().clamp(1.0, 2000.0) as i64
}

// (used, reset, observed time, smoothed percentage-points/second, last activity)
type Observation = (f64, i64, i64, f64, i64);
static OBSERVATIONS: Mutex<std::collections::BTreeMap<String, Observation>> =
    Mutex::new(std::collections::BTreeMap::new());

fn consumption_rate(index: &str, used: f64, reset: i64, t: i64) -> Option<f64> {
    let mut observations = OBSERVATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let old = observations
        .entry(index.to_owned())
        .or_insert((used, reset, t, 0.0, t));
    if reset != old.1 || used < old.0 {
        *old = (used, reset, t, 0.0, t);
    } else if t - old.2 >= 60 {
        let sample = (used - old.0) / (t - old.2) as f64;
        let activity = if sample > 0.0 { t } else { old.4 };
        let smoothed = if old.3 == 0.0 {
            sample
        } else {
            old.3 * 0.5 + sample * 0.5
        };
        *old = (used, reset, t, smoothed, activity);
    }
    (t - old.4 <= 600 && old.3 > 0.0).then_some(old.3)
}

pub fn capped(quota: &Value, limit: f64, _now: i64) -> Result<bool, String> {
    Ok(window_usage(quota, 18000)?.0 >= limit)
}

fn window_usage(quota: &Value, seconds: i64) -> Result<(f64, i64), String> {
    let mut found = None;
    for key in ["primary_window", "secondary_window"] {
        let w = &quota["rate_limit"][key];
        if w["limit_window_seconds"].as_i64() != Some(seconds) {
            continue;
        }
        if found.is_some() {
            return Err("Ambiguous configured quota window".into());
        }
        let used = w["used_percent"]
            .as_f64()
            .ok_or("Missing configured window usage")?;
        if !(0.0..=100.0).contains(&used) || w["reset_at"].as_i64().unwrap_or(0) <= 0 {
            return Err("Invalid configured quota window".into());
        }
        found = Some((used, w["reset_at"].as_i64().unwrap()));
    }
    found.ok_or("Missing configured quota window".into())
}

fn check(file: &Value, pacing: bool) -> Result<Value, String> {
    let index = file["auth_index"].as_str().ok_or("Missing auth index")?;
    let name = file["name"].as_str().ok_or("Missing auth filename")?;
    let mut auth = host("host.auth.get", json!({"auth_index":index}))?["json"].clone();
    let protected = auth.get("codex_quota_guard_limit").is_some()
        || auth.get("codex_quota_guard_weekly_limit").is_some();
    let pacing = pacing && auth["codex_quota_pacing"] == true;
    if !protected && !pacing {
        return Ok(json!({"auth_index":index,"protected":false,"blocked":false}));
    }
    let limit = auth
        .get("codex_quota_guard_limit")
        .map(|v| v.as_f64().unwrap_or(f64::NAN));
    let weekly_limit = auth
        .get("codex_quota_guard_weekly_limit")
        .map(|v| v.as_f64().unwrap_or(f64::NAN));
    let owned = auth["codex_quota_guard_paused"] == true;
    if (auth["disabled"] == true || file["disabled"] == true) && !owned {
        return Ok(
            json!({"auth_index":index,"protected":protected,"blocked":true,"status":"manually disabled; unchanged"}),
        );
    }
    let mut pacing_windows = None;
    let reading = (|| {
        if [limit, weekly_limit]
            .into_iter()
            .flatten()
            .any(|v| !(0.0..100.0).contains(&v))
        {
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
        let five = limit.map(|_| window_usage(&q, 18000)).transpose()?;
        let weekly = weekly_limit.map(|_| window_usage(&q, 604800)).transpose()?;
        if pacing {
            pacing_windows = window_usage(&q, 18000)
                .ok()
                .zip(window_usage(&q, 604800).ok());
        }
        let five_blocked = five
            .zip(limit)
            .is_some_and(|((used, _), threshold)| used >= threshold);
        let weekly_blocked = weekly
            .zip(weekly_limit)
            .is_some_and(|((used, _), threshold)| used >= threshold);
        Ok((
            five_blocked || weekly_blocked,
            five,
            weekly,
            if weekly_blocked {
                "weekly cutoff reached"
            } else if five_blocked {
                "five-hour cutoff reached"
            } else {
                "live quota below all configured cutoffs"
            },
        ))
    })();
    let (blocked, five, weekly, status) = match reading {
        Ok((b, f, w, s)) => (b, f, w, s.to_owned()),
        Err(e) => (protected, None, None, e),
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
    let mut report = json!({"auth_index":index,"protected":protected,"blocked":blocked,"limit_used_percent":limit,
        "five_hour_used_percent":five.map(|w|w.0),"reset_at":five.map(|w|w.1),
        "weekly_limit_used_percent":weekly_limit,"weekly_used_percent":weekly.map(|w|w.0),
        "weekly_reset_at":weekly.map(|w|w.1),"status":status,"checked_at":now()});
    if pacing && !blocked {
        let rate = pacing_windows.and_then(|(f, _)| consumption_rate(index, f.0, f.1, now()));
        let weight = pacing_windows.map_or(1, |(f, w)| {
            assignment_weight(
                [
                    (f.0, limit.unwrap_or(100.0), f.1, 18000),
                    (w.0, weekly_limit.unwrap_or(100.0), w.1, 604800),
                ],
                now(),
                rate,
            )
        });
        // Single writer shared with the reserve guard, through semantic host
        // updates; no competing priority writer or private session scheduler.
        auth = host("host.auth.get", json!({"auth_index":index}))?["json"].clone();
        if auth["disabled"] != true
            && auth["codex_quota_pacing"] == true
            && auth["weight"].as_i64() != Some(weight)
        {
            auth["weight"] = json!(weight);
            host("host.auth.save", json!({"name":name,"json":auth}))?;
        }
        report["pacing"] = json!({"weight":weight,"verified_windows":pacing_windows.is_some(),
            "five_hour_used_percent":pacing_windows.map(|w|w.0.0),
            "weekly_used_percent":pacing_windows.map(|w|w.1.0),
            "consumption_points_per_second":rate});
    }
    Ok(report)
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
        let result = check(f, true)
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
        let report = check(f, false)?;
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
                json!({"schema_version":6,"metadata":{"Name":ID,"Version":"0.3.0","Author":"Local quota policy","GitHubRepository":"https://github.com/Vikt0r70/codex-window-activation","ConfigFields":[]},
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

    #[test]
    fn pacing_subtracts_reserves_and_uses_both_windows() {
        // Incorrect full-quota accounting would over-assign the protected pool.
        let full = assignment_weight(
            [(70.0, 100.0, 19000, 18000), (50.0, 100.0, 605800, 604800)],
            1000,
            None,
        );
        let reserved = assignment_weight(
            [(70.0, 80.0, 19000, 18000), (50.0, 90.0, 605800, 604800)],
            1000,
            None,
        );
        assert!(reserved < full);
        assert!(
            assignment_weight(
                [(0.0, 100.0, 19000, 18000), (99.0, 100.0, 605800, 604800)],
                1000,
                None
            ) < full
        );
        assert_eq!(
            assignment_weight(
                [(80.0, 80.0, 19000, 18000), (0.0, 90.0, 605800, 604800)],
                1000,
                None
            ),
            1
        );
    }

    #[test]
    fn pacing_rewards_reset_opportunity_without_inventing_recovery() {
        let early = assignment_weight(
            [(20.0, 90.0, 2800, 18000), (10.0, 90.0, 605800, 604800)],
            1000,
            None,
        );
        let late = assignment_weight(
            [(20.0, 90.0, 19000, 18000), (10.0, 90.0, 605800, 604800)],
            1000,
            None,
        );
        assert!(early > late);
        assert_eq!(
            assignment_weight(
                [(80.0, 80.0, 900, 18000), (0.0, 90.0, 605800, 604800)],
                1000,
                None
            ),
            1
        );
        assert_eq!(
            assignment_weight(
                [(0.0, 80.0, 900, 18000), (0.0, 90.0, 605800, 604800)],
                1000,
                None
            ),
            1
        );
    }

    #[test]
    fn observed_consumption_reduces_new_work_without_zeroing_affinity() {
        let w = [(70.0, 90.0, 10000, 18000), (40.0, 90.0, 605800, 604800)];
        let idle = assignment_weight(w, 1000, None);
        // 20 spendable percentage points / 0.1 points per second = 200s runway.
        let busy = assignment_weight(w, 1000, Some(0.1));
        assert!(busy < idle);
        assert!(busy >= 1);
    }

    #[test]
    fn consumption_samples_need_time_and_expire_or_reset() {
        let id = "isolated-rate-test";
        assert_eq!(consumption_rate(id, 10.0, 20000, 1000), None);
        assert_eq!(consumption_rate(id, 11.0, 20000, 1010), None);
        assert_eq!(consumption_rate(id, 16.0, 20000, 1060), Some(0.1));
        assert_eq!(consumption_rate(id, 16.0, 20000, 1661), None);
        assert_eq!(consumption_rate(id, 0.0, 40000, 1670), None);
    }
}

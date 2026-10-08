use codex_window_activation::{decision, windows, Decision};
use serde_json::json;

#[test]
fn due_five_hour_window_activates_but_not_before_reset() {
    let q = json!({"rate_limit":{"allowed":true,"primary_window":{"limit_window_seconds":18000,"used_percent":0,"reset_at":1000},"secondary_window":{"limit_window_seconds":604800,"used_percent":20,"reset_at":10000}}});
    let w = windows(&q).unwrap();
    assert_eq!(decision(&w, 999, &json!({})), Decision::Wait);
    assert_eq!(decision(&w, 1031, &json!({})), Decision::Activate);
}

#[test]
fn simultaneous_resets_need_one_activation_and_completed_deadlines_dont_repeat() {
    let q = json!({"rate_limit":{"allowed":true,"primary_window":{"limit_window_seconds":18000,"used_percent":0,"reset_at":1000},"secondary_window":{"limit_window_seconds":604800,"used_percent":0,"reset_at":1000}}});
    let w = windows(&q).unwrap();
    assert_eq!(decision(&w, 1031, &json!({})), Decision::Activate);
    assert_eq!(
        decision(
            &w,
            1040,
            &json!({"completed":[1000],"last_attempt":1031,"retry_after":0})
        ),
        Decision::Wait
    );
}

#[test]
fn exhausted_other_window_and_failure_backoff_prevent_prompt() {
    let q = json!({"rate_limit":{"allowed":true,"primary_window":{"limit_window_seconds":18000,"used_percent":0,"reset_at":1000},"secondary_window":{"limit_window_seconds":604800,"used_percent":100,"reset_at":10000}}});
    let w = windows(&q).unwrap();
    assert_eq!(decision(&w, 1031, &json!({})), Decision::Exhausted);
    let safe = json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"used_percent":0,"reset_at":1000}}});
    assert_eq!(
        decision(&windows(&safe).unwrap(), 1031, &json!({"retry_after":2000})),
        Decision::Wait
    );
}

#[test]
fn active_new_window_never_gets_synthetic_prompt_and_swapped_windows_parse() {
    let q = json!({"rate_limit":{"primary_window":{"limit_window_seconds":604800,"used_percent":30,"reset_at":10000},"secondary_window":{"limit_window_seconds":18000,"used_percent":5,"reset_at":9000}}});
    let w = windows(&q).unwrap();
    assert_eq!(w.len(), 2);
    assert_eq!(decision(&w, 1031, &json!({})), Decision::Wait);
}

#[test]
fn malformed_or_unknown_quota_is_not_permission_to_spend() {
    assert!(windows(&json!({})).is_err());
    assert!(windows(&json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"used_percent":101,"reset_at":1000}}})).is_err());
}

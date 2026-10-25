//! Webhook queueing, signing and delivery.
//!
//! Two kinds of target get the same events:
//! - a payment's own `ipn_callback_url`: the flat payment object, signed with the project IPN secret;
//! - project endpoints from the dashboard: `{event, delivery_id, created_at, data}`, signed with the
//!   endpoint secret.
//!
//! Every delivery carries two signatures: the compatible HMAC-SHA512 of the key-sorted JSON in the
//! project's signature header, and `x-turmite-signature: v1=<hex HMAC-SHA256 of "<ts>.<body>">`.

use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::{Sha256, Sha512};

use crate::{
    app::App,
    db::{DueDelivery, PaymentRow},
    error::Result,
    ids,
};

pub const USER_AGENT: &str = "turmite-webhook/1";

/// Seconds to wait before attempt `n + 1` (n = attempts made). `None`: give up.
pub fn retry_delay(is_final: bool, attempts_made: i32) -> Option<i64> {
    const PROGRESS: &[i64] = &[10, 60];
    const FINAL: &[i64] = &[10, 60, 300, 1800, 3600, 7200, 14_400, 28_800, 86_400, 172_800];
    let schedule = if is_final { FINAL } else { PROGRESS };
    usize::try_from(attempts_made - 1).ok().and_then(|i| schedule.get(i).copied())
}

/// Queue webhooks for the payment's current state. The key holds the count of status changes:
/// a repeat of one change is dropped, a later return to the same status is sent.
pub async fn payment_changed(app: &App, p: &PaymentRow) -> Result<()> {
    let event_key = format!("payment:{}:{}:{}", p.payment_id, p.status_seq, p.status.as_str());
    let event = format!("payment.{}", p.status.as_str());
    enqueue_payment(app, p, &event, &event_key).await
}

/// Queue webhooks for a new `actually_paid` on a finished payment (a later deposit).
pub async fn payment_received(app: &App, p: &PaymentRow) -> Result<()> {
    let event_key = format!("payment:{}:{}:paid:{}", p.payment_id, p.status_seq, p.actually_paid.normalize());
    let event = format!("payment.{}", p.status.as_str());
    enqueue_payment(app, p, &event, &event_key).await
}

/// Queue webhooks for a credit on a payment whose status did not change
/// (top-up, overpayment, late deposit).
pub async fn payment_credited(app: &App, p: &PaymentRow, credited_total: &str) -> Result<()> {
    let event_key = format!("payment:{}:credit:{credited_total}", p.payment_id);
    let event = format!("payment.{}", p.status.as_str());
    enqueue_payment(app, p, &event, &event_key).await
}

async fn enqueue_payment(app: &App, p: &PaymentRow, event: &str, event_key: &str) -> Result<()> {
    let data = crate::payments::payment_json_full(app, p, crate::payments::PaymentShape::Webhook).await?;
    let is_final = p.status.is_terminal();
    if let Some(url) = p.ipn_callback_url.as_deref() {
        app.store.enqueue_delivery(p.project_id, None, url, event, event_key, is_final, &data, &ids::token()).await?;
    }
    for e in app.store.list_webhook_endpoints(p.project_id).await?.into_iter().filter(|e| e.is_enabled) {
        let token = ids::token();
        let envelope = json!({ "event": event, "delivery_id": token, "created_at": Utc::now().timestamp(), "data": data });
        app.store
            .enqueue_delivery(p.project_id, Some(e.webhook_endpoint_id), &e.url, event, event_key, is_final, &envelope, &token)
            .await?;
    }
    Ok(())
}

/// Queue an event for every endpoint of a project (payouts and other non-payment events).
pub async fn project_event(app: &App, project_id: i64, event: &str, event_key: &str, is_final: bool, data: &Value) -> Result<()> {
    for e in app.store.list_webhook_endpoints(project_id).await?.into_iter().filter(|e| e.is_enabled) {
        let token = ids::token();
        let envelope = json!({ "event": event, "delivery_id": token, "created_at": Utc::now().timestamp(), "data": data });
        app.store.enqueue_delivery(project_id, Some(e.webhook_endpoint_id), &e.url, event, event_key, is_final, &envelope, &token).await?;
    }
    Ok(())
}

/// Queue a `test` event for one endpoint.
pub async fn send_test(app: &App, project_id: i64, endpoint_id: i64) -> Result<()> {
    let endpoints = app.store.list_webhook_endpoints(project_id).await?;
    let Some(e) = endpoints.into_iter().find(|e| e.webhook_endpoint_id == endpoint_id) else {
        return Err(crate::error::Error::NotFound);
    };
    let token = ids::token();
    let envelope = json!({
        "event": "test",
        "delivery_id": token,
        "created_at": Utc::now().timestamp(),
        "data": { "message": "Test event from Turmite." },
    });
    app.store.enqueue_delivery(project_id, Some(endpoint_id), &e.url, "test", &format!("test:{token}"), false, &envelope, &token).await
}

/// HMAC-SHA512 over the JSON with object keys sorted at every level (the compatible signature).
pub fn sign_sorted(secret: &[u8], payload: &Value) -> String {
    sign_sorted_text(secret, &sorted_json(payload))
}

/// HMAC-SHA512 of text that [`sorted_json`] wrote.
fn sign_sorted_text(secret: &[u8], sorted: &str) -> String {
    let mut mac = <Hmac<Sha512>>::new_from_slice(secret).expect("any key length");
    mac.update(sorted.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// `v1=` + hex HMAC-SHA256 of `"<timestamp>.<body>"`.
pub fn sign_v1(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let mut mac = <Hmac<Sha256>>::new_from_slice(secret).expect("any key length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("v1={}", hex::encode(mac.finalize().into_bytes()))
}

/// JSON with object keys sorted recursively, no spaces, numbers as JavaScript writes them.
/// Webhook bodies are this text, so `JSON.stringify(sortKeys(JSON.parse(body)))` gives the body again.
pub fn sorted_json(v: &Value) -> String {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), sorted_json(&map[k])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => format!("[{}]", items.iter().map(sorted_json).collect::<Vec<_>>().join(",")),
        Value::Number(n) => js_number(n),
        other => other.to_string(),
    }
}

/// Largest integer that a JavaScript number holds exactly.
const JS_MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A JSON number as JavaScript's `Number.prototype.toString` writes it.
fn js_number(n: &serde_json::Number) -> String {
    if let Some(u) = n.as_u64().filter(|u| *u <= JS_MAX_SAFE_INTEGER) {
        return u.to_string();
    }
    if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= JS_MAX_SAFE_INTEGER) {
        return i.to_string();
    }
    n.as_f64().map(js_f64).unwrap_or_else(|| n.to_string())
}

/// ECMAScript Number::toString(x): the shortest digits that read back as `x`, in plain
/// decimals for 1e-7 <= |x| < 1e21 and in exponent form (`1e-7`, `1.5e-7`, `1e+21`) else.
pub fn js_f64(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        // JSON has no NaN or infinity; serde_json stores neither.
        return "0".into();
    }
    // Rust writes the shortest round-trip digits too: `d.ddde-7`.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let k = digits.len() as i32;
    // The value is 0.<digits> * 10^n.
    let n = exp.parse::<i32>().unwrap_or(0) + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n - 1 < 0 { '-' } else { '+' };
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { String::new() } else { format!(".{rest}") };
        format!("{first}{rest}e{sign}{}", (n - 1).abs())
    };
    if x < 0.0 { format!("-{body}") } else { body }
}

/// Deliver due webhooks. Returns how many were attempted.
pub async fn deliver_due(app: &App) -> Result<usize> {
    let due = app.store.claim_due_deliveries(25).await?;
    let n = due.len();
    let mut tasks = tokio::task::JoinSet::new();
    for d in due {
        let app = app.clone();
        tasks.spawn(async move {
            if let Err(e) = deliver_one(&app, &d).await {
                tracing::warn!(delivery = d.webhook_delivery_id, error = %e, "webhook delivery bookkeeping failed");
            }
        });
    }
    while tasks.join_next().await.is_some() {}
    Ok(n)
}

async fn deliver_one(app: &App, d: &DueDelivery) -> Result<()> {
    let context = if d.from_endpoint { "webhook_secret" } else { "ipn_secret" };
    let secret = app.sealer.open(context, &d.secret)?;
    // The body is the sorted text that the compatible signature covers.
    let sorted = sorted_json(&d.payload);
    let body = sorted.clone().into_bytes();
    let ts = Utc::now().timestamp();
    let attempt = async {
        let client = crate::netguard::webhook_client(app)?;
        crate::netguard::check_target(app, &d.url).await?;
        client
            .post(&d.url)
            .header("content-type", "application/json")
            .header("user-agent", USER_AGENT)
            .header(d.signature_header.as_str(), sign_sorted_text(&secret, &sorted))
            .header("x-turmite-timestamp", ts.to_string())
            .header("x-turmite-signature", sign_v1(&secret, ts, &body))
            .header("x-turmite-event", d.event_type.as_str())
            .header("x-turmite-delivery", d.delivery_token.as_str())
            .body(body.clone())
            .send()
            .await
            .map_err(|e| crate::error::Error::Driver(e.without_url().to_string()))
    };
    match attempt.await {
        Ok(r) if r.status().is_success() => app.store.mark_delivery_delivered(d.webhook_delivery_id, r.status().as_u16() as i32).await,
        Ok(r) => {
            let code = r.status().as_u16() as i32;
            let retry = retry_delay(d.is_final, d.attempt_count + 1);
            app.store.mark_delivery_failed(d.webhook_delivery_id, Some(code), &format!("HTTP {code}"), retry).await
        }
        Err(e) => {
            let retry = retry_delay(d.is_final, d.attempt_count + 1);
            app.store.mark_delivery_failed(d.webhook_delivery_id, None, &e.to_string(), retry).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorted_json_is_canonical() {
        let v = json!({"b": 1, "a": {"d": [2, {"z": null, "y": "x"}], "c": 1.5}});
        assert_eq!(sorted_json(&v), r#"{"a":{"c":1.5,"d":[2,{"y":"x","z":null}]},"b":1}"#);
    }

    #[test]
    fn numbers_print_as_in_javascript() {
        // Expected text: `String(x)` in Node.js.
        for (x, js) in [
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (0.00000123, "0.00000123"),
            (123456789.123, "123456789.123"),
            (1e21, "1e+21"),
            (1.2345e22, "1.2345e+22"),
            (1e20, "100000000000000000000"),
            (100.0, "100"),
            (0.1 + 0.2, "0.30000000000000004"),
            (-0.5, "-0.5"),
            (-0.0, "0"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (0.0012177, "0.0012177"),
        ] {
            assert_eq!(js_f64(x), js, "{x:e}");
        }
        let v = json!({
            "f": [0.000001, 0.0000001, 1.5e-7, 123456789.123, 1e21, 100.0],
            "i": [0, 7, -42, 9007199254740991u64, 18446744073709551615u64],
        });
        assert_eq!(
            sorted_json(&v),
            r#"{"f":[0.000001,1e-7,1.5e-7,123456789.123,1e+21,100],"i":[0,7,-42,9007199254740991,18446744073709552000]}"#
        );
    }

    #[test]
    fn sorted_json_reads_back_to_itself() {
        let v = json!({
            "z": "ünïcødé ✓ 😀 \"q\" \\ / \u{1}\n\u{2028}",
            "a": {"y": [1, {"d": 0.000001, "c": null}], "b": true},
            "m": 1.5e-7,
            "n": {},
            "e": [],
        });
        let text = sorted_json(&v);
        assert!(text.contains("ünïcødé ✓ 😀"), "non-ASCII text stays as it is: {text}");
        assert!(text.contains(r#"\"q\" \\ / \u0001\n"#), "{text}");
        let again: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(sorted_json(&again), text);
        assert_eq!(sign_sorted(b"k", &again), sign_sorted_text(b"k", &text));
    }

    /// The SDK and the plugin test with these bodies. The cases that a parsed object must also
    /// verify are bodies that this writer gives back unchanged.
    #[test]
    fn shared_signature_fixtures_match() {
        let cases: Vec<Value> = serde_json::from_str(include_str!("../../../sdk/typescript/test/fixtures/signatures.json")).unwrap();
        let mut checked = 0;
        for c in cases.iter().filter(|c| c["object_input_ok"] == true) {
            let body = c["body"].as_str().unwrap();
            let v: Value = serde_json::from_str(body).unwrap();
            assert_eq!(sorted_json(&v), body, "{}", c["name"]);
            assert_eq!(sign_sorted(c["secret"].as_str().unwrap().as_bytes(), &v), c["sig"].as_str().unwrap(), "{}", c["name"]);
            checked += 1;
        }
        assert!(checked >= 2);
    }

    #[test]
    fn retry_schedule() {
        assert_eq!(retry_delay(false, 1), Some(10));
        assert_eq!(retry_delay(false, 2), Some(60));
        assert_eq!(retry_delay(false, 3), None);
        assert_eq!(retry_delay(true, 10), Some(172_800));
        assert_eq!(retry_delay(true, 11), None);
    }

    #[test]
    fn v1_signature_format() {
        let s = sign_v1(b"secret", 1_700_000_000, b"{}");
        assert!(s.starts_with("v1=") && s.len() == 3 + 64);
    }
}

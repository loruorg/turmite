use askama::Template;
use axum::{
    Form, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Deserialize;

use super::{DashResult, Proj, ProjectRole, Shell, err_redirect, ok_redirect, page};
use crate::{
    app::App,
    crypto::random_hex,
    db::Page,
    http::views::{ago, status_class, status_label},
};

pub fn routes() -> Router<App> {
    Router::new()
        .route("/p/{pid}/webhooks", get(list).post(add))
        .route("/p/{pid}/webhooks/{eid}/delete", post(delete))
        .route("/p/{pid}/webhooks/{eid}/test", post(test))
        .route("/p/{pid}/webhooks/deliveries/{did}/retry", post(retry))
}

struct EndpointLine {
    id: i64,
    url: String,
    created: String,
}

struct DeliveryLine {
    id: i64,
    event: String,
    url: String,
    status: String,
    status_label: String,
    status_class: &'static str,
    attempts: i32,
    code: String,
    error: String,
    when: String,
    kind: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard/webhooks.html")]
struct WebhooksPage {
    shell: Shell,
    pid: i64,
    endpoints: Vec<EndpointLine>,
    deliveries: Vec<DeliveryLine>,
    new_secret: String,
    can_edit: bool,
    signature_header: String,
}

async fn view(app: &App, p: &Proj, new_secret: String) -> DashResult {
    let pid = p.project.project_id;
    let endpoints = app
        .store
        .list_webhook_endpoints(pid)
        .await?
        .into_iter()
        .map(|e| EndpointLine { id: e.webhook_endpoint_id, url: e.url, created: ago(e.created_at) })
        .collect();
    let deliveries = app
        .store
        .list_deliveries(pid, Page::new(50, 0))
        .await?
        .into_iter()
        .map(|d| DeliveryLine {
            id: d.webhook_delivery_id,
            event: d.event_type,
            url: d.url,
            status_label: status_label(&d.status),
            status_class: status_class(&d.status),
            status: d.status,
            attempts: d.attempt_count,
            code: d.last_status_code.map(|c| c.to_string()).unwrap_or_default(),
            error: d.last_error.unwrap_or_default(),
            when: ago(d.created_at),
            kind: if d.webhook_endpoint_id.is_some() { "Endpoint" } else { "IPN" },
        })
        .collect();
    let shell = Shell::new(app, &p.user, Some(&p.project), "webhooks", "Webhooks").await;
    Ok(page(
        app,
        &WebhooksPage {
            shell,
            pid,
            endpoints,
            deliveries,
            new_secret,
            can_edit: p.role >= ProjectRole::Developer,
            signature_header: p.project.signature_header.clone(),
        },
    ))
}

async fn list(State(app): State<App>, p: Proj) -> DashResult {
    view(&app, &p, String::new()).await
}

#[derive(Deserialize)]
struct AddForm {
    url: String,
}

async fn add(State(app): State<App>, p: Proj, Form(f): Form<AddForm>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    let back = format!("{}/webhooks", p.base(&app));
    let url = f.url.trim().to_string();
    if let Err(e) = crate::payments::check_url(&app, "url", &Some(url.clone())) {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    if let Err(e) = crate::netguard::check_target(&app, &url).await {
        return Ok(err_redirect(&app, &back, &e.public_message()));
    }
    let secret = random_hex(24);
    let id = app.store.add_webhook_endpoint(p.project.project_id, &url, &app.sealer.seal_str("webhook_secret", &secret)).await?;
    p.audit(&app, "webhook.add", "webhook_endpoint", Some(&id.to_string()), serde_json::json!({ "url": url })).await;
    view(&app, &p, secret).await
}

async fn delete(State(app): State<App>, p: Proj, Path((_pid, eid)): Path<(i64, i64)>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    app.store.delete_webhook_endpoint(p.project.project_id, eid).await?;
    p.audit(&app, "webhook.delete", "webhook_endpoint", Some(&eid.to_string()), serde_json::json!({})).await;
    Ok(ok_redirect(&app, &format!("{}/webhooks", p.base(&app)), "Endpoint deleted."))
}

async fn test(State(app): State<App>, p: Proj, Path((_pid, eid)): Path<(i64, i64)>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    crate::webhooks::send_test(&app, p.project.project_id, eid).await?;
    Ok(ok_redirect(&app, &format!("{}/webhooks", p.base(&app)), "Test event queued. It shows under Deliveries."))
}

async fn retry(State(app): State<App>, p: Proj, Path((_pid, did)): Path<(i64, i64)>) -> DashResult {
    p.require(ProjectRole::Developer)?;
    app.store.retry_delivery_now(p.project.project_id, did).await?;
    Ok(ok_redirect(&app, &format!("{}/webhooks", p.base(&app)), "Delivery scheduled."))
}

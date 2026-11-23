use crate::client;
use crate::daemon::Failure;
use crate::host::{self, Host, Store};
use crate::hub::{Hub, HubError, Payload, PREVIEW_NOT_FOUND};
use crate::records::Records;
use serde_json::{json, Value};
use serve_core::paths::Dirs;
use serve_core::record::{Found, Record};
use serve_core::{time, token, NAME};

pub struct Context<'a> {
    pub hub: &'a Hub,
    pub store: &'a Store,
    pub records: &'a Records,
    pub dirs: &'a Dirs,
}

pub fn sweep(ctx: &Context) {
    let now = time::iso(time::now_ms());
    let relay = client::find(ctx.dirs);
    for record in ctx.records.all() {
        let stale = match record.mode.as_str() {
            "persistent" => record
                .expires_at
                .as_deref()
                .is_some_and(|e| e <= now.as_str()),
            _ => relay.as_ref().map(|r| r.pid) != record.pid || record.pid.is_none(),
        };
        if stale {
            ctx.records.remove(&record.id);
        }
    }
}

fn hub_list(ctx: &Context, host: &Host) -> Result<Vec<Value>, HubError> {
    let value = ctx.hub.call(
        "GET",
        host::api(ctx.hub, host, "previews"),
        Some(&host.secret),
        Payload::None,
    )?;
    Ok(value
        .get("previews")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

fn mode_of(preview: &Value) -> &'static str {
    match (
        preview.get("mode").and_then(Value::as_str),
        preview.get("state").and_then(Value::as_str),
    ) {
        (Some("snapshot"), Some("uploading")) => "uploading",
        (Some("snapshot"), _) => "snapshot",
        _ => "live",
    }
}

fn row(preview: &Value, record: Option<&Record>) -> Value {
    let text = |k: &str| preview.get(k).and_then(Value::as_str).map(str::to_string);
    json!({
        "previewToken": text("previewToken"),
        "mode": mode_of(preview),
        "url": record.and_then(|r| r.url.clone()).or_else(|| text("url")),
        "localUrl": record.and_then(|r| r.local_url.clone()),
        "cdpUrl": record.and_then(|r| r.cdp_url.clone()),
        "root": record.and_then(|r| r.root.clone()),
        "createdAt": text("createdAt"),
        "expiresAt": text("expiresAt"),
        "foreground": record.is_some_and(|r| r.foreground),
    })
}

pub fn gather(ctx: &Context) -> Result<Vec<Value>, Failure> {
    sweep(ctx);
    let records = ctx.records.all();
    let mut rows = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if let Some(host) = ctx.store.saved(ctx.hub) {
        let previews = match hub_list(ctx, &host) {
            Ok(p) => p,
            Err(e) if e.recreates() => {
                ctx.store.forget(&host.host_id);
                if matches!(e, HubError::Refused { .. }) {
                    return Err(Failure::hub(&e, "listing previews"));
                }
                Vec::new()
            }
            Err(e) => return Err(Failure::hub(&e, "listing previews")),
        };
        for preview in &previews {
            let token = preview
                .get("previewToken")
                .and_then(Value::as_str)
                .unwrap_or("");
            seen.push(token.to_string());
            let record = records
                .iter()
                .find(|r| r.preview_token.as_deref() == Some(token));
            rows.push(row(preview, record));
        }
    }
    for record in &records {
        match (record.mode.as_str(), record.preview_token.as_deref()) {
            ("local", _) => rows.push(json!({
                "previewToken": Value::Null,
                "id": record.id,
                "mode": "local",
                "url": record.local_url,
                "localUrl": record.local_url,
                "root": record.root,
                "createdAt": record.created_at,
                "expiresAt": Value::Null,
                "foreground": record.foreground,
            })),
            (_, Some(t)) if !seen.iter().any(|s| s == t) => ctx.records.remove(&record.id),
            _ => {}
        }
    }
    Ok(rows)
}

pub enum Stopped {
    Token(String),
    Local(String),
}

pub fn stop(ctx: &Context, target: &str) -> Result<Stopped, Failure> {
    let record = match ctx.records.find(target) {
        Found::None => None,
        Found::One(record) => Some(*record),
        Found::Many(n) => {
            return Err(Failure::plain(format!(
                "{NAME}: {target} matches {n} previews; stop one by its token"
            )))
        }
    };
    let token = record
        .as_ref()
        .and_then(|r| r.preview_token.clone())
        .or_else(|| token::from_url(target))
        .or_else(|| token::is_token(target).then(|| target.to_string()));
    if let (Some(record), None) = (&record, &token) {
        let stopped = client::find(ctx.dirs)
            .filter(|relay| Some(relay.pid) == record.pid)
            .and_then(|relay| client::send(&relay, json!({ "op": "stop", "id": record.id })).ok())
            .is_some_and(|x| x.reply.get("ok") == Some(&json!(true)));
        if !stopped {
            ctx.records.remove(&record.id);
        }
        return Ok(Stopped::Local(
            record
                .local_url
                .clone()
                .unwrap_or_else(|| record.id.clone()),
        ));
    }
    let Some(token) = token else {
        return Err(Failure::plain(format!(
            "{NAME}: no preview matches {target}"
        )));
    };
    let host = ctx
        .store
        .saved(ctx.hub)
        .filter(|h| h.host_id == token::host_of(&token));
    let Some(host) = host else {
        return Err(Failure::plain(format!(
            "{NAME}: {token} is not a preview of this machine's host"
        )));
    };
    let answer = ctx.hub.call(
        "DELETE",
        host::api(ctx.hub, &host, &format!("previews/{token}")),
        Some(&host.secret),
        Payload::None,
    );
    for r in ctx.records.all() {
        if r.preview_token.as_deref() == Some(token.as_str()) {
            ctx.records.remove(&r.id);
        }
    }
    match answer {
        Ok(_) => Ok(Stopped::Token(token)),
        Err(HubError::Gone {
            code: Some(code), ..
        }) if code == PREVIEW_NOT_FOUND => Err(Failure::plain(format!(
            "{NAME}: the serve hub has no preview {token} (already stopped or expired)"
        ))),
        Err(e) => {
            if e.recreates() {
                ctx.store.forget(&host.host_id);
            }
            Err(Failure::hub(&e, "stopping the preview"))
        }
    }
}

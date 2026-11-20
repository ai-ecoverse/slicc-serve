use crate::client;
use crate::daemon::Failure;
use crate::hub::{Hub, HubError, Payload};
use crate::records::Records;
use crate::tray::{self, Store, Tray};
use serde_json::{json, Value};
use serve_core::paths::Dirs;
use serve_core::record::Record;
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

fn hub_list(ctx: &Context, tray: &Tray) -> Result<Vec<Value>, HubError> {
    let value = ctx.hub.call(
        "GET",
        tray::api(ctx.hub, tray, "previews"),
        Some(&tray.controller_token),
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
        (Some("persistent"), Some("pending")) => "uploading",
        (Some("persistent"), _) => "snapshot",
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
        "root": record.and_then(|r| r.root.clone()),
        "createdAt": text("createdAt"),
        "expiresAt": text("expiresAt"),
        "foreground": record.is_some_and(|r| r.foreground),
    })
}

pub fn gather(ctx: &Context) -> Result<Vec<Value>, Failure> {
    sweep(ctx);
    let records = ctx.records.all();
    let file = ctx.store.load();
    let origin = ctx.hub.origin();
    let mut rows = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let current = file.current.clone().filter(|t| t.hub == origin);
    let trays: Vec<(Tray, bool)> = current
        .into_iter()
        .map(|t| (t, true))
        .chain(
            file.retired
                .iter()
                .filter(|t| t.hub == origin)
                .cloned()
                .map(|t| (t, false)),
        )
        .collect();
    for (tray, is_current) in trays {
        let previews = match hub_list(ctx, &tray) {
            Ok(p) => p,
            Err(e) if e.recreates() => {
                let keep = matches!(e, HubError::Gone { .. })
                    && ctx.records.holds_snapshots(&tray.tray_id);
                drop_tray(ctx, &tray, is_current, keep);
                if is_current && matches!(e, HubError::Refused { .. }) {
                    return Err(Failure::hub(&e, "listing previews"));
                }
                continue;
            }
            Err(e) => return Err(Failure::hub(&e, "listing previews")),
        };
        if !is_current && !previews.iter().any(|p| mode_of(p) != "live") {
            ctx.store.forget(&tray.tray_id);
        }
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

fn drop_tray(ctx: &Context, tray: &Tray, is_current: bool, keep: bool) {
    let id = tray.tray_id.clone();
    let _ = ctx.store.update(|f| {
        if is_current && f.current.as_ref().is_some_and(|t| t.tray_id == id) {
            let old = f.current.take();
            if keep {
                f.retired.extend(old);
            }
        } else if !keep {
            f.retired.retain(|t| t.tray_id != id);
        }
    });
}

pub fn table(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "no previews\n".into();
    }
    let cell = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("-").to_string();
    let lines: Vec<[String; 5]> = std::iter::once([
        "TOKEN".to_string(),
        "MODE".into(),
        "EXPIRES".into(),
        "URL".into(),
        "ROOT".into(),
    ])
    .chain(rows.iter().map(|r| {
        let token = r
            .get("previewToken")
            .and_then(Value::as_str)
            .or_else(|| r.get("id").and_then(Value::as_str))
            .unwrap_or("-")
            .to_string();
        let mode = if r.get("foreground") == Some(&json!(true)) {
            format!("{} (foreground)", cell(r, "mode"))
        } else {
            cell(r, "mode")
        };
        let root = match (
            r.get("root").and_then(Value::as_str),
            r.get("localUrl").and_then(Value::as_str),
        ) {
            (Some(root), _) => root.to_string(),
            (None, Some(local)) if cell(r, "mode") != "local" => local.to_string(),
            _ => "-".into(),
        };
        [token, mode, cell(r, "expiresAt"), cell(r, "url"), root]
    }))
    .collect();
    let widths: Vec<usize> = (0..4)
        .map(|i| {
            lines
                .iter()
                .map(|l| l[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for l in &lines {
        for (i, w) in widths.iter().enumerate() {
            out.push_str(&format!("{:<w$}  ", l[i]));
        }
        out.push_str(&l[4]);
        out.push('\n');
    }
    out
}

pub enum Stopped {
    Token(String),
    Local(String),
}

pub fn stop(ctx: &Context, target: &str) -> Result<Stopped, Failure> {
    let record = ctx.records.find(target);
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
    let file = ctx.store.load();
    let Some(tray) = file.find(token::tray_of(&token)).cloned() else {
        return Err(Failure::plain(format!(
            "{NAME}: {token} is not a preview of this machine's trays"
        )));
    };
    let is_current = file
        .current
        .as_ref()
        .is_some_and(|t| t.tray_id == tray.tray_id);
    let answer = ctx.hub.call(
        "POST",
        tray::api(ctx.hub, &tray, "preview/stop"),
        Some(&tray.controller_token),
        Payload::Json(json!({ "previewToken": token })),
    );
    for r in ctx.records.all() {
        if r.preview_token.as_deref() == Some(token.as_str()) {
            ctx.records.remove(&r.id);
        }
    }
    match answer {
        Ok(v) if v.get("revoked") == Some(&json!(true)) => Ok(Stopped::Token(token)),
        Ok(_) => Err(Failure::plain(format!(
            "{NAME}: the tray hub has no preview {token} (already stopped or expired)"
        ))),
        Err(e) => {
            if e.recreates() {
                let keep = matches!(e, HubError::Gone { .. })
                    && ctx.records.holds_snapshots(&tray.tray_id);
                drop_tray(ctx, &tray, is_current, keep);
            }
            Err(Failure::hub(&e, "stopping the preview"))
        }
    }
}

use super::embedder::{self, Error, ENOTSUP};
use super::out::{self, Failure};
use super::store::Store;
use super::sys;
use serde_json::{json, Value};
use serve_core::record::{self, Found, Record};
use serve_core::{time, token};

fn row(preview: &Value, record: Option<&Record>) -> Value {
    let text = |k: &str| preview.get(k).and_then(Value::as_str).map(str::to_string);
    let since = preview
        .get("since")
        .and_then(Value::as_u64)
        .map(time::iso)
        .or_else(|| text("createdAt"));
    let snapshot = text("mode").as_deref() == Some("persistent");
    let expires = preview
        .get("expiresAt")
        .and_then(Value::as_u64)
        .map(time::iso)
        .or_else(|| record.and_then(|r| r.expires_at.clone()));
    json!({
        "previewToken": text("previewToken"),
        "mode": if snapshot { "snapshot" } else { "live" },
        "url": record.and_then(|r| r.url.clone()).or_else(|| text("url")),
        "localUrl": record.and_then(|r| r.local_url.clone()),
        "root": record.and_then(|r| r.root.clone()).or_else(|| text("root")),
        "createdAt": record.map(|r| r.created_at.clone()).or(since),
        "expiresAt": if snapshot { json!(expires) } else { Value::Null },
        "foreground": record.is_some_and(|r| r.foreground),
    })
}

fn local_row(record: &Record) -> Value {
    let live = record.preview_token.is_some();
    let mode = match (record.mode.as_str(), live) {
        ("persistent", _) => "snapshot",
        (_, true) => "live",
        _ => "local",
    };
    json!({
        "previewToken": record.preview_token,
        "id": record.id,
        "mode": mode,
        "url": if live { record.url.clone() } else { record.local_url.clone() },
        "localUrl": record.local_url,
        "root": record.root,
        "createdAt": record.created_at,
        "expiresAt": record.expires_at,
        "foreground": record.foreground,
    })
}

fn stop_holder(store: &Store, record: &Record) {
    if let Some(pid) = record.pid.filter(|p| *p != sys::pid() && sys::alive(*p)) {
        let _ = sys::signal(pid, sys::SIGTERM);
    }
    store.remove(&record.id);
}

pub fn list(cmd: &str, json: bool) -> i32 {
    let store = Store::new();
    let records = store.living();
    let previews = match embedder::exchange(&json!({ "kind": "serve.list" })) {
        Ok(reply) => Some(
            reply
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .filter(Value::is_object)
                .collect::<Vec<Value>>(),
        ),
        Err(e) if matches!(e, Error::Missing) || e.errno() == Some(ENOTSUP) => None,
        Err(e) => return out::fail(&Failure::page(cmd, &e), json),
    };
    let mut rows = Vec::new();
    match previews {
        Some(previews) => {
            let token_of = |p: &Value| {
                p.get("previewToken")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            };
            for preview in &previews {
                let token = token_of(preview);
                let found = records
                    .iter()
                    .find(|r| r.preview_token.is_some() && r.preview_token == token);
                rows.push(row(preview, found));
            }
            for record in &records {
                match &record.preview_token {
                    None => rows.push(local_row(record)),
                    Some(t) if !previews.iter().any(|p| token_of(p).as_ref() == Some(t)) => {
                        stop_holder(&store, record)
                    }
                    Some(_) => {}
                }
            }
        }
        None => rows.extend(records.iter().map(local_row)),
    }
    if json {
        println!("{}", Value::Array(rows));
    } else {
        print!("{}", serve_core::table::render(&rows));
    }
    0
}

pub fn stop(cmd: &str, target: &str, json: bool) -> i32 {
    let store = Store::new();
    let record = match record::find(store.living(), target) {
        Found::None => None,
        Found::One(r) => Some(*r),
        Found::Many(n) => {
            let line = format!("{cmd}: {target} matches {n} previews; stop one by its token");
            return out::fail(&Failure::plain(line), json);
        }
    };
    let token = record
        .as_ref()
        .and_then(|r| r.preview_token.clone())
        .or_else(|| token::from_url(target))
        .or_else(|| token::is_token(target).then(|| target.to_string()));
    let stopped = match (&token, &record) {
        (Some(token), _) => {
            let asked = embedder::tell(&json!({ "kind": "serve.stop", "previewToken": token }));
            if let Err(e) = asked {
                if let Some(record) = &record {
                    stop_holder(&store, record);
                }
                return out::fail(&Failure::page(cmd, &e), json);
            }
            if let Some(record) = &record {
                store.remove(&record.id);
            }
            token.clone()
        }
        (None, Some(record)) => {
            stop_holder(&store, record);
            record
                .local_url
                .clone()
                .unwrap_or_else(|| record.id.clone())
        }
        (None, None) => {
            return out::fail(
                &Failure::plain(format!("{cmd}: no preview matches {target}")),
                json,
            )
        }
    };
    if json {
        println!("{}", json!({ "stopped": stopped }));
    } else {
        println!("stopped {stopped}");
    }
    0
}

fn target_token(cmd: &str, target: Option<&str>, json: bool) -> Result<Option<String>, i32> {
    let Some(target) = target else {
        return Ok(None);
    };
    let found = match record::find(Store::new().living(), target) {
        Found::One(r) => r.preview_token,
        _ => None,
    };
    match found
        .or_else(|| token::from_url(target))
        .or_else(|| token::is_token(target).then(|| target.to_string()))
    {
        Some(t) => Ok(Some(t)),
        None => Err(out::fail(
            &Failure::plain(format!("{cmd}: no preview matches {target}")),
            json,
        )),
    }
}

pub fn logs(cmd: &str, target: Option<&str>, lines: Option<usize>, json: bool) -> i32 {
    let token = match target_token(cmd, target, json) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let mut request = json!({ "kind": "serve.logs" });
    if let Some(t) = &token {
        request["previewToken"] = json!(t);
    }
    let reply = match embedder::exchange(&request) {
        Ok(r) => r,
        Err(e) => return out::fail(&Failure::page(cmd, &e), json),
    };
    let mut records: Vec<Value> = reply
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(Value::is_object)
        .collect();
    if let Some(n) = lines {
        let skip = records.len().saturating_sub(n);
        records.drain(..skip);
    }
    if json {
        println!("{}", Value::Array(records));
    } else if records.is_empty() {
        println!(
            "no lifecycle records (seven keeps them in memory, so they are gone when it reloads)"
        );
    } else {
        println!("Preview lifecycle records (oldest to newest):");
        for r in &records {
            println!("{}", serve_core::table::log_line(r));
        }
    }
    0
}

pub fn truncate(cmd: &str, target: Option<&str>, json: bool) -> i32 {
    let token = match target_token(cmd, target, json) {
        Ok(t) => t,
        Err(code) => return code,
    };
    let mut request = json!({ "kind": "serve.truncate" });
    if let Some(t) = &token {
        request["previewToken"] = json!(t);
    }
    let reply = match embedder::exchange(&request) {
        Ok(r) => r,
        Err(e) => return out::fail(&Failure::page(cmd, &e), json),
    };
    let answer: Value = reply
        .lines()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null);
    let count = |k: &str| answer.get(k).and_then(Value::as_u64).unwrap_or(0);
    let (cleared, rearmed) = (count("cleared"), count("rearmed"));
    if json {
        println!("{}", json!({ "cleared": cleared, "rearmed": rearmed }));
    } else {
        println!("cleared {cleared} records, re-armed {rearmed} previews");
    }
    0
}

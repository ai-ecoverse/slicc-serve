use super::embedder;
use super::out::{self, Failure};
use super::store::{random_hex, Store};
use serde_json::{json, Value};
use serve_core::args::Serve;
use serve_core::record::{public_url, Record};
use serve_core::{snapshot, time};

pub fn publish(cmd: &str, dir: &str, ttl_ms: u64, opts: &Serve) -> i32 {
    match run(cmd, dir, ttl_ms, opts) {
        Ok((record, files, bytes)) => out::snapshot_done(cmd, &record, opts, Some((files, bytes))),
        Err(f) => out::fail(&f, opts.json),
    }
}

fn run(cmd: &str, dir: &str, ttl_ms: u64, opts: &Serve) -> Result<(Record, usize, u64), Failure> {
    let files = snapshot::collect(std::path::Path::new(dir))
        .map_err(|e| Failure::plain(format!("{cmd}: {e}")))?;
    let entry_rel = opts.entry.as_deref().unwrap_or("index.html");
    if !files.iter().any(|f| f.path == entry_rel) {
        return Err(Failure::plain(format!(
            "{cmd}: {dir} has no {entry_rel}; a --ttl snapshot needs its entry file (give one with --entry)"
        )));
    }
    let bytes: u64 = files.iter().map(|f| f.size).sum();
    let mut request = json!({
        "kind": "serve.snapshot",
        "root": dir,
        "ttlMs": ttl_ms,
        "files": files.len(),
        "bytes": bytes,
    });
    if let Some(entry) = &opts.entry {
        request["entry"] = json!(entry);
    }
    let reply = embedder::snapshot(&request, &files).map_err(|e| Failure::page(cmd, &e))?;
    let line = reply.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.is_empty() {
        return Err(Failure::plain(format!(
            "{cmd}: seven ended the request without a URL"
        )));
    }
    let answer: Value = serde_json::from_str(line).unwrap_or(Value::Null);
    if answer.get("error").is_some() {
        let doing = match answer.get("during").and_then(Value::as_str) {
            Some("upload" | "finalize") => "uploading the snapshot",
            _ => "publishing the snapshot",
        };
        return Err(Failure::hub(cmd, &answer, doing));
    }
    let text = |k: &str| answer.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(url), Some(token)) = (text("url"), text("previewToken")) else {
        return Err(Failure::plain(format!(
            "{cmd}: seven's answer to serve.snapshot has no url or previewToken"
        )));
    };
    let now = time::now_ms();
    let expires = answer
        .get("expiresAt")
        .and_then(Value::as_u64)
        .unwrap_or(now + ttl_ms);
    let record = Record {
        id: random_hex(8),
        mode: "persistent".into(),
        preview_token: Some(token),
        url: Some(public_url(&url, opts.entry.is_some())),
        root: Some(dir.to_string()),
        entry: opts.entry.clone(),
        host_id: text("hostId"),
        created_at: time::iso(now),
        expires_at: Some(time::iso(expires)),
        ..Record::default()
    };
    if let Err(e) = Store::new().save(&record) {
        eprintln!("{cmd}: can't save the record: {e}");
    }
    Ok((record, files.len(), bytes))
}

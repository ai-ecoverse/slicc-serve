use crate::daemon::Failure;
use crate::fsx;
use crate::host::{self, Host, Store};
use crate::hub::{Hub, HubError, Payload};
use crate::records::Records;
use serde_json::{json, Value};
use serve_core::record::{public_url, Record};
use serve_core::snapshot::{self, File};
use serve_core::{safe, time, NAME, SERVED_PREFIX};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const UPLOADS: usize = 4;

pub struct Snapshot {
    pub record: Record,
    pub files: usize,
    pub bytes: u64,
}

fn upload_all(hub: &Hub, host: &Host, token: &str, files: &[File]) -> Result<(), HubError> {
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<HubError>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..UPLOADS.min(files.len()) {
            scope.spawn(|| loop {
                if failure.lock().unwrap().is_some() {
                    return;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(file) = files.get(i) else { return };
                let result = std::fs::read(&file.abs)
                    .map_err(|e| HubError::Protocol {
                        message: format!("can't read {}: {e}", file.path),
                    })
                    .and_then(|bytes| {
                        let path = safe::url_path(&file.path);
                        let url = host::api(hub, host, &format!("previews/{token}/files{path}"));
                        hub.call(
                            "PUT",
                            url,
                            Some(&host.secret),
                            Payload::Bytes(bytes, file.mime.to_string()),
                        )
                    });
                if let Err(e) = result {
                    let mut slot = failure.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(match e {
                            HubError::Protocol { .. } => e,
                            other => tag(other, &file.path),
                        });
                    }
                    return;
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn tag(error: HubError, path: &str) -> HubError {
    match error {
        HubError::Client {
            status,
            message,
            code,
        } => HubError::Client {
            status,
            message: format!("{message} ({path})"),
            code,
        },
        HubError::Gone {
            status,
            message,
            code,
        } => HubError::Gone {
            status,
            message: format!("{message} ({path})"),
            code,
        },
        other => other,
    }
}

pub fn publish(
    hub: &Hub,
    store: &Store,
    records: &Records,
    dir: &Path,
    entry: Option<&str>,
    ttl_ms: u64,
) -> Result<Snapshot, Failure> {
    let files = snapshot::collect(dir).map_err(|e| Failure::plain(format!("{NAME}: {e}")))?;
    let entry_rel = entry.unwrap_or("index.html");
    if !files.iter().any(|f| f.path == entry_rel) {
        return Err(Failure::plain(format!(
            "{NAME}: {} has no {entry_rel}; a --ttl snapshot needs its entry file (give one with --entry)",
            dir.display()
        )));
    }
    let id = fsx::random_hex(8);
    let tag = format!("{SERVED_PREFIX}{id}");
    let body = json!({
        "entry": entry.map_or_else(|| "/".into(), safe::url_path),
        "tag": tag,
        "ttlMs": ttl_ms,
    });
    let (value, host) = store
        .with_host(hub, |host| {
            hub.call(
                "POST",
                host::api(hub, host, "previews"),
                Some(&host.secret),
                Payload::Json(body.clone()),
            )
        })
        .map_err(|e| Failure::hub(&e, "publishing the snapshot"))?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let Some(token) = text(&value, "previewToken") else {
        return Err(Failure::hub(
            &HubError::Protocol {
                message: "the serve hub's mint answer has no previewToken".into(),
            },
            "publishing the snapshot",
        ));
    };
    let finished = upload_all(hub, &host, &token, &files).and_then(|_| {
        hub.call(
            "POST",
            host::api(hub, &host, &format!("previews/{token}/finalize")),
            Some(&host.secret),
            Payload::None,
        )
    });
    let done = match finished {
        Ok(v) => v,
        Err(e) => {
            let _ = hub.call(
                "DELETE",
                host::api(hub, &host, &format!("previews/{token}")),
                Some(&host.secret),
                Payload::None,
            );
            return Err(Failure::hub(&e, "uploading the snapshot"));
        }
    };
    let Some(url) = text(&done, "url").or_else(|| text(&value, "url")) else {
        let _ = hub.call(
            "DELETE",
            host::api(hub, &host, &format!("previews/{token}")),
            Some(&host.secret),
            Payload::None,
        );
        return Err(Failure::hub(
            &HubError::Protocol {
                message: "the serve hub's finalize answer has no url".into(),
            },
            "uploading the snapshot",
        ));
    };
    let now = time::now_ms();
    let expires = done
        .get("expiresAt")
        .and_then(|v| {
            v.as_u64()
                .map(time::iso)
                .or_else(|| v.as_str().map(str::to_string))
        })
        .unwrap_or_else(|| time::iso(now + ttl_ms));
    let record = Record {
        id,
        mode: "persistent".into(),
        preview_token: Some(token),
        url: Some(public_url(&url, entry.is_some())),
        root: Some(dir.display().to_string()),
        entry: entry.map(str::to_string),
        host_id: Some(host.host_id.clone()),
        tag: Some(tag),
        created_at: time::iso(now),
        expires_at: Some(expires),
        ..Record::default()
    };
    let _ = records.save(&record);
    Ok(Snapshot {
        files: files.len(),
        bytes: files.iter().map(|f| f.size).sum(),
        record,
    })
}

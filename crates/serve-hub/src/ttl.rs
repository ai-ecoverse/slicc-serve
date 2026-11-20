use crate::daemon::Failure;
use crate::fsx;
use crate::hub::{Hub, HubError, Payload};
use crate::records::Records;
use crate::tray::{self, Store, Tray};
use serde_json::{json, Value};
use serve_core::record::{public_url, Record};
use serve_core::snapshot::{self, File};
use serve_core::{time, NAME, SERVED_PREFIX};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

pub const UPLOADS: usize = 4;

pub struct Snapshot {
    pub record: Record,
    pub files: usize,
    pub bytes: u64,
}

struct Minted {
    token: String,
    upload: String,
}

fn upload_all(hub: &Hub, tray: &Tray, minted: &Minted, files: &[File]) -> Result<(), HubError> {
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
                        let mut url =
                            tray::api(hub, tray, &format!("preview/{}/file", minted.token));
                        url.query_pairs_mut().append_pair("path", &file.path);
                        hub.call(
                            "PUT",
                            url,
                            Some(&minted.upload),
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
    let served_root = format!("{SERVED_PREFIX}{id}");
    let entry_path = format!("{served_root}/{entry_rel}");
    let body = json!({
        "servedRoot": served_root,
        "entryPath": entry_path,
        "allowLive": false,
        "ttlMs": ttl_ms,
    });
    let keep = |t: &Tray| records.holds_snapshots(&t.tray_id);
    let (value, tray) = store
        .with_tray(hub, &keep, |tray| {
            hub.call(
                "POST",
                tray::api(hub, tray, "preview"),
                Some(&tray.controller_token),
                Payload::Json(body.clone()),
            )
        })
        .map_err(|e| Failure::hub(&e, "publishing the snapshot"))?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(token), Some(upload)) = (text(&value, "previewToken"), text(&value, "uploadToken"))
    else {
        return Err(Failure::hub(
            &HubError::Protocol {
                message: "the tray hub's mint answer has no upload capability".into(),
            },
            "publishing the snapshot",
        ));
    };
    let minted = Minted { token, upload };
    let finished = upload_all(hub, &tray, &minted, &files).and_then(|_| {
        hub.call(
            "POST",
            tray::api(hub, &tray, &format!("preview/{}/finalize", minted.token)),
            Some(&minted.upload),
            Payload::None,
        )
    });
    let done = match finished {
        Ok(v) => v,
        Err(e) => {
            let _ = hub.call(
                "POST",
                tray::api(hub, &tray, "preview/stop"),
                Some(&tray.controller_token),
                Payload::Json(json!({ "previewToken": minted.token })),
            );
            return Err(Failure::hub(&e, "uploading the snapshot"));
        }
    };
    let url = text(&done, "url")
        .or_else(|| text(&value, "url"))
        .unwrap_or_default();
    let now = time::now_ms();
    let record = Record {
        id,
        mode: "persistent".into(),
        preview_token: Some(minted.token),
        url: Some(public_url(&url, entry.is_some())),
        root: Some(dir.display().to_string()),
        entry: entry.map(str::to_string),
        tray_id: Some(tray.tray_id.clone()),
        served_root: Some(served_root),
        created_at: time::iso(now),
        expires_at: Some(time::iso(now + ttl_ms)),
        ..Record::default()
    };
    let _ = records.save(&record);
    Ok(Snapshot {
        files: files.len(),
        bytes: files.iter().map(|f| f.size).sum(),
        record,
    })
}

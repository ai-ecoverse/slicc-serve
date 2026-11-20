use crate::fsx;
use crate::hub::{Hub, HubError, Payload};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serve_core::time;
use std::path::PathBuf;

pub const RUNTIME: &str = "slicc-serve/1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tray {
    pub hub: String,
    pub tray_id: String,
    pub controller_token: String,
    pub controller_url: String,
    pub controller_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader_key: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrayFile {
    #[serde(default)]
    pub current: Option<Tray>,
    #[serde(default)]
    pub retired: Vec<Tray>,
}

impl TrayFile {
    pub fn all(&self) -> impl Iterator<Item = &Tray> {
        self.current.iter().chain(self.retired.iter())
    }

    pub fn find(&self, tray_id: &str) -> Option<&Tray> {
        self.all().find(|t| t.tray_id == tray_id)
    }
}

#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(config: PathBuf) -> Store {
        Store { dir: config }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("tray.json")
    }

    pub fn load(&self) -> TrayFile {
        fsx::read_json(&self.path()).unwrap_or_default()
    }

    pub fn update<T>(&self, change: impl FnOnce(&mut TrayFile) -> T) -> std::io::Result<T> {
        let _lock = fsx::lock(&self.dir.join("tray.lock"))?;
        let mut file = self.load();
        let out = change(&mut file);
        fsx::write_json(&self.path(), &file)?;
        Ok(out)
    }

    pub fn saved(&self, hub: &Hub) -> Option<Tray> {
        self.load().current.filter(|t| t.hub == hub.origin())
    }

    pub fn remember(&self, tray: &Tray) {
        let _ = self.update(|f| {
            for t in f.current.iter_mut().chain(f.retired.iter_mut()) {
                if t.tray_id == tray.tray_id {
                    *t = tray.clone();
                }
            }
        });
    }

    pub fn current(&self, hub: &Hub) -> Result<(Tray, bool), HubError> {
        if let Some(tray) = self.saved(hub) {
            return Ok((tray, false));
        }
        let tray = create(hub)?;
        self.install(&tray, None, false);
        Ok((tray, true))
    }

    fn install(&self, tray: &Tray, old: Option<&Tray>, keep_old: bool) {
        let _ = self.update(|f| {
            if let Some(prev) = f.current.take() {
                let dropped = old.is_some_and(|o| o.tray_id == prev.tray_id) && !keep_old;
                let known = f.retired.iter().any(|t| t.tray_id == prev.tray_id);
                if !dropped && !known && prev.tray_id != tray.tray_id {
                    f.retired.push(prev);
                }
            }
            if let (Some(old), false) = (old, keep_old) {
                f.retired.retain(|t| t.tray_id != old.tray_id);
            }
            f.current = Some(tray.clone());
        });
    }

    pub fn replace(&self, hub: &Hub, old: &Tray, keep_old: bool) -> Result<Tray, HubError> {
        let tray = create(hub)?;
        self.install(&tray, Some(old), keep_old);
        Ok(tray)
    }

    pub fn forget(&self, tray_id: &str) {
        let _ = self.update(|f| f.retired.retain(|t| t.tray_id != tray_id));
    }

    pub fn with_tray<T>(
        &self,
        hub: &Hub,
        keep: &dyn Fn(&Tray) -> bool,
        mut op: impl FnMut(&Tray) -> Result<T, HubError>,
    ) -> Result<(T, Tray), HubError> {
        let (tray, fresh) = self.current(hub)?;
        match op(&tray) {
            Ok(v) => Ok((v, tray)),
            Err(e) if e.recreates() && !fresh => {
                let keep_old = matches!(e, HubError::Gone { .. }) && keep(&tray);
                let tray = self.replace(hub, &tray, keep_old)?;
                op(&tray).map(|v| (v, tray))
            }
            Err(e) => Err(e),
        }
    }
}

pub fn create(hub: &Hub) -> Result<Tray, HubError> {
    let value = hub.call("POST", hub.url("/tray"), None, Payload::Json(json!({})))?;
    let text = |v: &Value, path: &[&str]| -> Option<String> {
        path.iter()
            .try_fold(v, |v, k| v.get(*k))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let bad = || HubError::Protocol {
        message: "the tray hub's answer to POST /tray has no trayId or controller capability"
            .into(),
    };
    Ok(Tray {
        hub: hub.origin(),
        tray_id: text(&value, &["trayId"]).ok_or_else(bad)?,
        controller_token: text(&value, &["capabilities", "controller", "token"]).ok_or_else(bad)?,
        controller_url: text(&value, &["capabilities", "controller", "url"]).ok_or_else(bad)?,
        controller_id: uuid(),
        leader_key: None,
        created_at: time::iso(time::now_ms()),
    })
}

fn uuid() -> String {
    let hex = fsx::random_hex(16);
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

pub fn claim(hub: &Hub, tray: &Tray) -> Result<(Tray, String), HubError> {
    let mut body = json!({ "controllerId": tray.controller_id, "runtime": RUNTIME });
    if let Some(key) = &tray.leader_key {
        body["leaderKey"] = json!(key);
    }
    let answer = hub
        .call(
            "POST",
            hub.here(&tray.controller_url),
            None,
            Payload::Json(body),
        )
        .map_err(|e| match e {
            HubError::Client {
                status: 409,
                message,
                code,
            } if code.as_deref() == Some("LEADER_KEY_REQUIRED") => HubError::Gone {
                status: 409,
                message,
            },
            other => other,
        })?;
    let role = answer.get("role").and_then(Value::as_str).unwrap_or("");
    let socket = answer
        .get("websocket")
        .and_then(|w| w.get("url"))
        .and_then(Value::as_str);
    let (Some(socket), "leader") = (socket, role) else {
        return Err(HubError::Gone {
            status: 409,
            message: "the tray hub didn't make this machine the leader of its own tray".into(),
        });
    };
    let mut tray = tray.clone();
    if let Some(key) = answer.get("leaderKey").and_then(Value::as_str) {
        tray.leader_key = Some(key.to_string());
    }
    let mut url = hub.here(socket);
    let scheme = if hub.base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    let _ = url.set_scheme(scheme);
    Ok((tray, url.to_string()))
}

pub fn api(hub: &Hub, tray: &Tray, tail: &str) -> url::Url {
    hub.url(&format!("/api/tray/{}/{tail}", tray.tray_id))
}

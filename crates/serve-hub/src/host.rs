use crate::fsx;
use crate::hub::{Hub, HubError, Payload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serve_core::time;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub hub: String,
    pub host_id: String,
    pub secret: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HostFile {
    #[serde(default)]
    pub hosts: Vec<Host>,
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
        self.dir.join("host.json")
    }

    pub fn load(&self) -> HostFile {
        fsx::read_json(&self.path()).unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut HostFile)) {
        let Ok(_lock) = fsx::lock(&self.dir.join("host.lock")) else {
            return;
        };
        let mut file = self.load();
        change(&mut file);
        let _ = fsx::write_json(&self.path(), &file);
    }

    pub fn saved(&self, hub: &Hub) -> Option<Host> {
        let origin = hub.origin();
        self.load().hosts.into_iter().find(|h| h.hub == origin)
    }

    pub fn current(&self, hub: &Hub) -> Result<(Host, bool), HubError> {
        match self.saved(hub) {
            Some(host) => Ok((host, false)),
            None => self.replace(hub, None).map(|h| (h, true)),
        }
    }

    pub fn replace(&self, hub: &Hub, old: Option<&Host>) -> Result<Host, HubError> {
        let _lock = fsx::lock(&self.dir.join("host.lock")).map_err(|e| HubError::Protocol {
            message: format!("can't lock {}: {e}", self.path().display()),
        })?;
        let mut file = self.load();
        let origin = hub.origin();
        if let Some(theirs) = file.hosts.iter().find(|h| h.hub == origin) {
            if old.is_none_or(|o| o.host_id != theirs.host_id) {
                return Ok(theirs.clone());
            }
        }
        let host = create(hub)?;
        file.hosts.retain(|h| h.hub != origin);
        file.hosts.push(host.clone());
        fsx::write_json(&self.path(), &file).map_err(|e| HubError::Protocol {
            message: format!("can't write {}: {e}", self.path().display()),
        })?;
        Ok(host)
    }

    pub fn forget(&self, host_id: &str) {
        let id = host_id.to_string();
        self.update(move |f| f.hosts.retain(|h| h.host_id != id));
    }

    pub fn with_host<T>(
        &self,
        hub: &Hub,
        mut op: impl FnMut(&Host) -> Result<T, HubError>,
    ) -> Result<(T, Host), HubError> {
        let (host, fresh) = self.current(hub)?;
        match op(&host) {
            Ok(v) => Ok((v, host)),
            Err(e) if e.recreates() && !fresh => {
                let host = self.replace(hub, Some(&host))?;
                op(&host).map(|v| (v, host))
            }
            Err(e) => Err(e),
        }
    }
}

pub fn create(hub: &Hub) -> Result<Host, HubError> {
    let value = hub.call("POST", hub.url("/api/serve/host"), None, Payload::None)?;
    let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
    match (text("hostId"), text("secret")) {
        (Some(host_id), Some(secret)) => Ok(Host {
            hub: hub.origin(),
            host_id,
            secret,
            created_at: time::iso(time::now_ms()),
        }),
        _ => Err(HubError::Protocol {
            message: "the serve hub's answer to POST /api/serve/host has no hostId or secret"
                .into(),
        }),
    }
}

pub fn api(hub: &Hub, host: &Host, tail: &str) -> url::Url {
    let tail = if tail.is_empty() {
        String::new()
    } else {
        format!("/{tail}")
    };
    hub.url(&format!("/api/serve/host/{}{tail}", host.host_id))
}

pub fn socket_url(hub: &Hub, host: &Host) -> String {
    let mut url = api(hub, host, "ws");
    let scheme = if hub.base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    let _ = url.set_scheme(scheme);
    url.to_string()
}

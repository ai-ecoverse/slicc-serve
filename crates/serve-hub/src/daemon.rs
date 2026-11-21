use crate::bridge::Bridge;
use crate::cdp::{self, Endpoint};
use crate::fsx;
use crate::hub::{Hub, HubError, Payload};
use crate::records::Records;
use crate::relay::{self, Origin};
use crate::tray::{self, Store, Tray};
use crate::ws;
use serde_json::{json, Value};
use serve_core::exit::Exit;
use serve_core::paths::Dirs;
use serve_core::record::{public_url, Record};
use serve_core::server::{self, Site};
use serve_core::{time, NAME, SERVED_PREFIX};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tungstenite::Message;

pub const IDLE: Duration = Duration::from_secs(5);
const PING: Duration = Duration::from_secs(30);
const RELINK: Duration = Duration::from_secs(3);
const RECONNECT: [u64; 5] = [1, 2, 5, 10, 30];

pub struct Failure {
    pub exit: Exit,
    pub lines: Vec<String>,
    pub sign_in: Option<String>,
}

impl Failure {
    pub fn plain(line: String) -> Failure {
        Failure {
            exit: Exit::Failure,
            lines: vec![line],
            sign_in: None,
        }
    }

    pub fn hub(error: &HubError, doing: &str) -> Failure {
        Failure {
            exit: error.exit(),
            lines: error.lines(doing),
            sign_in: error.sign_in().map(str::to_string),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "ok": false, "exit": self.exit.code(), "lines": self.lines, "signIn": self.sign_in })
    }
}

struct ServerHandle {
    stop: Arc<AtomicBool>,
    port: u16,
}

impl ServerHandle {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

struct Entry {
    record: Record,
    origin: Option<Origin>,
    server: Option<ServerHandle>,
    holder: Option<TcpStream>,
}

struct Link {
    tray: Tray,
    alive: Arc<AtomicBool>,
}

pub struct Daemon {
    hub: Hub,
    store: Store,
    records: Records,
    state: PathBuf,
    token: String,
    entries: Mutex<HashMap<String, Entry>>,
    link: Mutex<Option<Link>>,
    connecting: Mutex<()>,
    generation: AtomicU64,
    busy: AtomicUsize,
    activity: Mutex<Instant>,
    agent: ureq::Agent,
    bridge: Arc<Bridge>,
    endpoint: Mutex<Option<Endpoint>>,
    info: Mutex<Value>,
    me: Weak<Daemon>,
}

pub fn hub_from_env() -> String {
    std::env::var("SLICC_TRAY_HUB")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| serve_core::DEFAULT_HUB.to_string())
}

pub fn run(dirs: Dirs) -> i32 {
    let hub = match Hub::new(&hub_from_env()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{NAME} relay: {e}");
            return 1;
        }
    };
    if fsx::private_dir(&dirs.state).is_err() {
        return 1;
    }
    let Ok(lock) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dirs.state.join("relay.lock"))
    else {
        return 1;
    };
    if lock.try_lock().is_err() {
        return 0;
    }
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
        return 1;
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let daemon = Arc::new_cyclic(|me| Daemon {
        me: me.clone(),
        store: Store::new(dirs.config.clone()),
        records: Records::new(&dirs.state),
        state: dirs.state.clone(),
        token: fsx::random_hex(24),
        entries: Mutex::new(HashMap::new()),
        link: Mutex::new(None),
        connecting: Mutex::new(()),
        generation: AtomicU64::new(0),
        busy: AtomicUsize::new(0),
        activity: Mutex::new(Instant::now()),
        agent: relay::agent(),
        bridge: Arc::new(Bridge::new(serve_core::version())),
        endpoint: Mutex::new(None),
        info: Mutex::new(Value::Null),
        hub,
    });
    daemon.sweep_records();
    let info = json!({
        "pid": std::process::id(),
        "port": port,
        "token": daemon.token,
        "version": serve_core::version(),
        "hub": daemon.hub.origin(),
    });
    *daemon.info.lock().unwrap() = info.clone();
    if fsx::write_json(&dirs.state.join("relay.json"), &info).is_err() {
        return 1;
    }
    eprintln!(
        "{NAME} relay {} for {} on 127.0.0.1:{port}",
        serve_core::version(),
        daemon.hub.origin()
    );
    let watchdog = daemon.clone();
    std::thread::spawn(move || watchdog.watch());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let d = daemon.clone();
        std::thread::spawn(move || d.control(stream));
    }
    drop(lock);
    0
}

fn line(stream: &mut TcpStream, value: &Value) -> bool {
    let mut text = value.to_string();
    text.push('\n');
    stream.write_all(text.as_bytes()).is_ok()
}

impl Daemon {
    fn touch(&self) {
        *self.activity.lock().unwrap() = Instant::now();
    }

    fn sweep_records(&self) {
        for record in self.records.all() {
            if record.mode != "persistent" {
                self.records.remove(&record.id);
            }
        }
    }

    fn watch(self: Arc<Self>) {
        loop {
            std::thread::sleep(Duration::from_millis(250));
            let idle = self.entries.lock().unwrap().is_empty()
                && self.busy.load(Ordering::SeqCst) == 0
                && self.activity.lock().unwrap().elapsed() > IDLE;
            if idle {
                let info = self.state.join("relay.json");
                let ours = fsx::read_json::<Value>(&info)
                    .and_then(|v| v.get("pid").and_then(Value::as_u64))
                    == Some(u64::from(std::process::id()));
                if ours {
                    let _ = std::fs::remove_file(info);
                }
                eprintln!("{NAME} relay: nothing left to serve, exiting");
                std::process::exit(0);
            }
        }
    }

    fn control(self: Arc<Self>, mut stream: TcpStream) {
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.touch();
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let mut reader = BufReader::new(stream.try_clone().expect("socket clone"));
        let mut text = String::new();
        let ok = reader.by_ref().take(1 << 20).read_line(&mut text).is_ok();
        let request: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let authorized = request.get("token").and_then(Value::as_str) == Some(self.token.as_str());
        if ok && authorized {
            match request.get("op").and_then(Value::as_str) {
                Some("ping") => {
                    line(
                        &mut stream,
                        &json!({ "ok": true, "pid": std::process::id(), "hub": self.hub.origin() }),
                    );
                }
                Some("publish") => self.clone().publish_op(&request, stream, reader),
                Some("logs") => {
                    let token = request.get("previewToken").and_then(Value::as_str);
                    let known = token.is_none_or(|t| self.bridge.known(t));
                    line(
                        &mut stream,
                        &json!({ "ok": true, "known": known, "records": self.bridge.logs(token) }),
                    );
                }
                Some("truncate") => {
                    let token = request.get("previewToken").and_then(Value::as_str);
                    let (cleared, rearmed) = self.bridge.truncate(token);
                    line(
                        &mut stream,
                        &json!({ "ok": true, "cleared": cleared, "rearmed": rearmed }),
                    );
                }
                Some("stop") => {
                    let id = request.get("id").and_then(Value::as_str).unwrap_or("");
                    let found = self.stop(id, "stopped with scserve --stop", true);
                    line(&mut stream, &json!({ "ok": found }));
                }
                _ => {
                    line(
                        &mut stream,
                        &json!({ "ok": false, "exit": 1, "lines": ["unknown op"] }),
                    );
                }
            }
        }
        self.touch();
        self.busy.fetch_sub(1, Ordering::SeqCst);
    }

    fn publish_op(
        self: Arc<Self>,
        request: &Value,
        mut stream: TcpStream,
        mut reader: BufReader<TcpStream>,
    ) {
        let hold = request
            .get("hold")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let wanted = request.get("hub").and_then(Value::as_str).unwrap_or("");
        let result = if !wanted.is_empty() && wanted != self.hub.origin() {
            Err(Failure::plain(format!(
                "{NAME}: a relay for another tray hub ({}) is running; stop its previews first",
                self.hub.origin()
            )))
        } else {
            self.publish(request, hold)
        };
        let record = match result {
            Ok(record) => record,
            Err(failure) => {
                line(&mut stream, &failure.to_json());
                return;
            }
        };
        line(&mut stream, &json!({ "ok": true, "record": record }));
        if !hold {
            return;
        }
        if let Ok(holder) = stream.try_clone() {
            if let Some(entry) = self.entries.lock().unwrap().get_mut(&record.id) {
                entry.holder = Some(holder);
            } else {
                return;
            }
        }
        let _ = stream.set_read_timeout(None);
        let mut sink = [0u8; 256];
        loop {
            match reader.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        self.stop(&record.id, "the foreground scserve ended", true);
    }

    fn start_site(&self, dir: &str) -> Result<(u16, ServerHandle), Failure> {
        let site = Site::new(std::path::Path::new(dir))
            .map_err(|e| Failure::plain(format!("{NAME}: {dir}: {e}")))?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| Failure::plain(format!("{NAME}: can't listen on 127.0.0.1: {e}")))?;
        let port = listener
            .local_addr()
            .map(|a| a.port())
            .map_err(|e| Failure::plain(format!("{NAME}: {e}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let site = Arc::new(site);
        let log: server::Log = Arc::new(|_, _| {});
        std::thread::spawn(move || server::serve(listener, site, flag, log));
        Ok((port, ServerHandle { stop, port }))
    }

    fn cdp_url(&self) -> Result<String, Failure> {
        let mut slot = self.endpoint.lock().unwrap();
        if let Some(e) = slot.as_ref() {
            return Ok(e.url());
        }
        let endpoint = cdp::start(self.bridge.clone(), fsx::random_hex(16))
            .map_err(|e| Failure::plain(format!("{NAME}: can't start the CDP endpoint: {e}")))?;
        let url = endpoint.url();
        let mut info = self.info.lock().unwrap();
        info["cdpPort"] = json!(endpoint.port);
        info["cdpSecret"] = json!(endpoint.secret);
        let _ = fsx::write_json(&self.state.join("relay.json"), &*info);
        *slot = Some(endpoint);
        Ok(url)
    }

    fn publish(&self, request: &Value, hold: bool) -> Result<Record, Failure> {
        let text = |k: &str| request.get(k).and_then(Value::as_str).map(str::to_string);
        let local = request
            .get("local")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let entry = text("entry");
        let explicit = entry.is_some();
        let bridged = !local
            && request
                .get("bridge")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let quiet = request
            .get("quiet")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_tabs = request.get("maxTabs").and_then(Value::as_u64).unwrap_or(20);
        let cdp_url = if bridged { Some(self.cdp_url()?) } else { None };
        let id = fsx::random_hex(8);
        let dir = text("dir");
        let (port, server) = match &dir {
            Some(dir) => {
                let (port, server) = self.start_site(dir)?;
                (port, Some(server))
            }
            None => (
                request
                    .get("port")
                    .and_then(Value::as_u64)
                    .and_then(|p| u16::try_from(p).ok())
                    .ok_or_else(|| Failure::plain(format!("{NAME}: no directory and no port")))?,
                None,
            ),
        };
        let mut record = Record {
            id: id.clone(),
            mode: if local { "local" } else { "live" }.into(),
            local_url: Some(format!(
                "http://127.0.0.1:{port}/{}",
                entry.as_deref().unwrap_or("")
            )),
            root: dir.clone(),
            port: Some(port),
            entry: entry.clone(),
            created_at: time::iso(time::now_ms()),
            pid: Some(std::process::id()),
            foreground: hold,
            bridge: bridged,
            cdp_url,
            ..Record::default()
        };
        let served_root = format!("{SERVED_PREFIX}{id}");
        let entry_path = format!("{served_root}/{}", entry.as_deref().unwrap_or("index.html"));
        let origin = (!local).then(|| Origin {
            port,
            served_root: served_root.clone(),
            entry_path: entry_path.clone(),
            entry_is_root: !explicit,
        });
        self.entries.lock().unwrap().insert(
            id.clone(),
            Entry {
                record: record.clone(),
                origin,
                server,
                holder: None,
            },
        );
        if !local {
            let mut body = json!({ "servedRoot": served_root, "entryPath": entry_path, "allowLive": dir.is_none() });
            if bridged {
                body["bridge"] = json!(true);
                body["maxTabs"] = json!(max_tabs);
                body["quiet"] = json!(quiet);
                body["webhookId"] = json!(format!("scserve-{id}"));
            }
            match self.mint(&body) {
                Ok((token, url, tray_id)) => {
                    if bridged {
                        let title = dir.clone().unwrap_or_else(|| format!("127.0.0.1:{port}"));
                        self.bridge
                            .register(&token, &public_url(&url, explicit), &title, quiet);
                    }
                    record.preview_token = Some(token);
                    record.url = Some(public_url(&url, explicit));
                    record.tray_id = Some(tray_id);
                    record.served_root = Some(served_root);
                }
                Err(e) => {
                    if let Some(entry) = self.entries.lock().unwrap().remove(&id) {
                        if let Some(server) = entry.server {
                            server.stop();
                        }
                    }
                    return Err(Failure::hub(&e, "publishing"));
                }
            }
        }
        if let Some(entry) = self.entries.lock().unwrap().get_mut(&id) {
            entry.record = record.clone();
        }
        let _ = self.records.save(&record);
        Ok(record)
    }

    fn mint(&self, body: &Value) -> Result<(String, String, String), HubError> {
        let call = |tray: &Tray| {
            self.hub.call(
                "POST",
                tray::api(&self.hub, tray, "preview"),
                Some(&tray.controller_token),
                Payload::Json(body.clone()),
            )
        };
        let tray = self.ensure_link()?;
        let answer = match call(&tray) {
            Err(e) if e.recreates() => {
                self.drop_link();
                let keep = matches!(e, HubError::Gone { .. })
                    && self.records.holds_snapshots(&tray.tray_id);
                let fresh = self.store.replace(&self.hub, &tray, keep)?;
                let tray = {
                    let _guard = self.connecting.lock().unwrap();
                    self.connect(fresh)?
                };
                call(&tray).map(|v| (v, tray))
            }
            other => other.map(|v| (v, tray)),
        };
        let (value, tray) = answer?;
        let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
        match (text("previewToken"), text("url")) {
            (Some(token), Some(url)) => Ok((token, url, tray.tray_id)),
            _ => Err(HubError::Protocol {
                message: "the tray hub's mint answer has no previewToken or url".into(),
            }),
        }
    }

    fn ensure_link(&self) -> Result<Tray, HubError> {
        let _guard = self.connecting.lock().unwrap();
        if let Some(link) = self.link.lock().unwrap().as_ref() {
            if link.alive.load(Ordering::SeqCst) {
                return Ok(link.tray.clone());
            }
        }
        let (tray, fresh) = self.store.current(&self.hub)?;
        match self.connect(tray.clone()) {
            Err(e) if e.recreates() && !fresh => {
                let keep = matches!(e, HubError::Gone { .. })
                    && self.records.holds_snapshots(&tray.tray_id);
                let tray = self.store.replace(&self.hub, &tray, keep)?;
                self.connect(tray)
            }
            other => other,
        }
    }

    fn drop_link(&self) {
        if let Some(link) = self.link.lock().unwrap().take() {
            link.alive.store(false, Ordering::SeqCst);
        }
    }

    fn connect(&self, tray: Tray) -> Result<Tray, HubError> {
        let (tray, url) = tray::claim(&self.hub, &tray)?;
        self.store.remember(&tray);
        let socket = ws::connect(&self.hub, &url)?;
        let (out, rx) = channel();
        self.bridge.relinking();
        self.bridge.set_hub(out.clone());
        let me = self.self_arc();
        std::thread::spawn(move || {
            std::thread::sleep(RELINK);
            for ev in me.bridge.drop_stale() {
                me.tell(&ev);
            }
        });
        let alive = Arc::new(AtomicBool::new(true));
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let previous = self.link.lock().unwrap().replace(Link {
            tray: tray.clone(),
            alive: alive.clone(),
        });
        if let Some(previous) = previous {
            previous.alive.store(false, Ordering::SeqCst);
        }
        eprintln!("{NAME} relay: leading tray {}", tray.tray_id);
        let me = self.self_arc();
        let t = tray.clone();
        std::thread::spawn(move || me.pump(socket, rx, out, alive, t, generation));
        let me = self.self_arc();
        let t = tray.clone();
        std::thread::spawn(move || me.tidy(&t));
        Ok(tray)
    }

    fn self_arc(&self) -> Arc<Daemon> {
        self.me.upgrade().expect("the daemon outlives its threads")
    }

    fn tidy(&self, tray: &Tray) {
        let Ok(list) = self.hub.call(
            "GET",
            tray::api(&self.hub, tray, "previews"),
            Some(&tray.controller_token),
            Payload::None,
        ) else {
            return;
        };
        let served: Vec<String> = self
            .entries
            .lock()
            .unwrap()
            .values()
            .filter_map(|e| e.origin.as_ref().map(|o| o.served_root.clone()))
            .collect();
        for preview in list
            .get("previews")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let mode = preview
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("live");
            let root = preview
                .get("servedRoot")
                .and_then(Value::as_str)
                .unwrap_or("");
            let token = preview.get("previewToken").and_then(Value::as_str);
            if mode == "persistent" || served.iter().any(|s| s == root) {
                continue;
            }
            if let Some(token) = token {
                eprintln!("{NAME} relay: revoking {token}, which nothing serves any more");
                let _ = self.hub.call(
                    "POST",
                    tray::api(&self.hub, tray, "preview/stop"),
                    Some(&tray.controller_token),
                    Payload::Json(json!({ "previewToken": token })),
                );
            }
        }
    }

    fn pump(
        self: Arc<Self>,
        mut socket: ws::Socket,
        rx: Receiver<String>,
        out: Sender<String>,
        alive: Arc<AtomicBool>,
        tray: Tray,
        generation: u64,
    ) {
        let mut pinged = Instant::now();
        let mut broken = false;
        while alive.load(Ordering::SeqCst) && !broken {
            while let Ok(text) = rx.try_recv() {
                if socket.send(Message::text(text)).is_err() {
                    broken = true;
                    break;
                }
            }
            if pinged.elapsed() > PING {
                pinged = Instant::now();
                if socket
                    .send(Message::text(json!({"type": "ping"}).to_string()))
                    .is_err()
                {
                    broken = true;
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => self.dispatch(text.as_str(), &out),
                Ok(Message::Close(_)) => broken = true,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => broken = true,
            }
        }
        let _ = socket.close(None);
        let _ = socket.flush();
        alive.store(false, Ordering::SeqCst);
        if !broken || self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        eprintln!("{NAME} relay: lost the tray hub, reconnecting");
        self.reconnect(tray, generation);
    }

    fn public_entries(&self) -> bool {
        self.entries
            .lock()
            .unwrap()
            .values()
            .any(|e| e.origin.is_some())
    }

    fn reconnect(&self, tray: Tray, generation: u64) {
        for attempt in 0.. {
            let delay = RECONNECT[attempt.min(RECONNECT.len() - 1)];
            std::thread::sleep(Duration::from_secs(delay));
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            if !self.public_entries() {
                self.drop_link();
                return;
            }
            let _guard = self.connecting.lock().unwrap();
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            let latest = self
                .store
                .load()
                .find(&tray.tray_id)
                .cloned()
                .unwrap_or(tray.clone());
            match self.connect(latest) {
                Ok(_) => return,
                Err(e) if e.recreates() => {
                    eprintln!("{NAME} relay: the tray hub dropped tray {}", tray.tray_id);
                    self.drop_link();
                    let ids: Vec<String> = self
                        .entries
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(_, e)| e.origin.is_some())
                        .map(|(id, _)| id.clone())
                        .collect();
                    for id in ids {
                        self.stop(&id, "the tray hub dropped this machine's tray", false);
                    }
                    return;
                }
                Err(e) => {
                    for l in e.lines("reconnecting") {
                        eprintln!("{l}");
                    }
                }
            }
        }
    }

    fn dispatch(&self, text: &str, out: &Sender<String>) {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match message.get("type").and_then(Value::as_str) {
            Some("preview.request") => {
                let Some(request) = relay::parse(&message) else {
                    return;
                };
                let (origin, holder) = {
                    let entries = self.entries.lock().unwrap();
                    let found = entries.values().find(|e| {
                        e.origin
                            .as_ref()
                            .is_some_and(|o| o.served_root == request.served_root)
                    });
                    (
                        found.and_then(|e| e.origin.clone()),
                        found.and_then(|e| e.holder.as_ref().and_then(|h| h.try_clone().ok())),
                    )
                };
                let out = out.clone();
                let agent = self.agent.clone();
                std::thread::spawn(move || {
                    let answer = relay::answer(&request, origin.as_ref(), &agent);
                    for m in &answer.messages {
                        let _ = out.send(m.to_string());
                    }
                    if let Some(mut holder) = holder {
                        let log = format!("GET {} {} {}", answer.path, answer.status, answer.bytes);
                        line(&mut holder, &json!({ "log": log }));
                    }
                });
            }
            Some("bridge.connected") => {
                if let Some(ev) = self.bridge.connected(&message) {
                    self.tell(&ev);
                }
            }
            Some("bridge.disconnected") => {
                if let Some(ev) = self.bridge.disconnected(&message) {
                    self.tell(&ev);
                }
            }
            Some("webhook.event") => {
                let ours = message
                    .get("webhookId")
                    .and_then(Value::as_str)
                    .is_some_and(|w| w.starts_with("scserve-"));
                if let Some(ev) = ours.then(|| self.bridge.emitted(&message)).flatten() {
                    self.tell(&ev);
                }
            }
            Some("bridge.cdp.response") => self.bridge.responded(&message),
            Some("preview.state") => self.bridge.preview_state(&message),
            Some("preview.revoked") => {
                let token = message
                    .get("previewToken")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = self
                    .entries
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, e)| e.record.preview_token.as_deref() == Some(token))
                    .map(|(id, _)| id.clone());
                if let Some(id) = id {
                    self.stop(&id, "revoked on the tray hub", false);
                }
            }
            _ => {}
        }
    }

    fn tell(&self, ev: &Value) {
        let token = ev.get("previewToken").and_then(Value::as_str);
        let holder = self
            .entries
            .lock()
            .unwrap()
            .values()
            .find(|e| e.record.preview_token.as_deref() == token)
            .and_then(|e| e.holder.as_ref().and_then(|h| h.try_clone().ok()));
        if let Some(mut holder) = holder {
            line(&mut holder, &json!({ "event": ev }));
        }
    }

    fn stop(&self, id: &str, reason: &str, revoke: bool) -> bool {
        let Some(entry) = self.entries.lock().unwrap().remove(id) else {
            return false;
        };
        if let (true, Some(token)) = (entry.record.bridge, entry.record.preview_token.as_deref()) {
            self.bridge.unregister(token);
        }
        self.touch();
        if let Some(server) = &entry.server {
            server.stop();
        }
        if let (true, Some(token), Some(tray_id)) = (
            revoke,
            entry.record.preview_token.as_deref(),
            entry.record.tray_id.as_deref(),
        ) {
            if let Some(tray) = self.store.load().find(tray_id).cloned() {
                let _ = self.hub.call(
                    "POST",
                    tray::api(&self.hub, &tray, "preview/stop"),
                    Some(&tray.controller_token),
                    Payload::Json(json!({ "previewToken": token })),
                );
            }
        }
        self.records.remove(id);
        if let Some(mut holder) = entry.holder {
            line(&mut holder, &json!({ "stopped": reason }));
            let _ = holder.shutdown(Shutdown::Both);
        }
        eprintln!("{NAME} relay: stopped {id}: {reason}");
        true
    }
}

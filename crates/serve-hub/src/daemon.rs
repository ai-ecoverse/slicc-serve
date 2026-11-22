use crate::bridge::Bridge;
use crate::cdp::{self, Endpoint};
use crate::fsx;
use crate::host::{self, Host, Store};
use crate::hub::{Hub, HubError, Payload};
use crate::records::Records;
use crate::relay::{self, Stream};
use crate::ws;
use serde_json::{json, Value};
use serve_core::exit::Exit;
use serve_core::paths::Dirs;
use serve_core::record::{public_url, Record};
use serve_core::server::{self, Site};
use serve_core::{safe, time, NAME, SERVED_PREFIX};
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

#[derive(Clone)]
struct Origin {
    port: u16,
    tag: String,
}

struct Entry {
    record: Record,
    origin: Option<Origin>,
    server: Option<ServerHandle>,
    holder: Option<TcpStream>,
}

struct Link {
    host: Host,
    alive: Arc<AtomicBool>,
}

struct Conn {
    out: Sender<Message>,
    streams: Mutex<HashMap<u32, Arc<Stream>>>,
}

impl Conn {
    fn text(&self, value: Value) {
        let _ = self.out.send(Message::text(value.to_string()));
    }

    fn stream(&self, id: u32) -> Option<Arc<Stream>> {
        self.streams.lock().unwrap().get(&id).cloned()
    }

    fn cancel_all(&self) {
        for (_, stream) in self.streams.lock().unwrap().drain() {
            stream.cancel();
        }
    }
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
    bridge: Arc<Bridge>,
    endpoint: Mutex<Option<Endpoint>>,
    info: Mutex<Value>,
    me: Weak<Daemon>,
}

pub fn hub_from_env() -> String {
    std::env::var("SLICC_SERVE_HUB")
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
                "{NAME}: a relay for another serve hub ({}, pid {}) is running; its previews end when it stops",
                self.hub.origin(),
                std::process::id()
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
        let tag = format!("{SERVED_PREFIX}{id}");
        let origin = (!local).then(|| Origin {
            port,
            tag: tag.clone(),
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
            let path = entry.as_deref().map_or_else(|| "/".into(), safe::url_path);
            let mut body = json!({ "entry": path, "tag": tag });
            if bridged {
                body["bridge"] = json!(true);
                body["maxTabs"] = json!(max_tabs);
                body["quiet"] = json!(quiet);
            }
            match self.mint(&body) {
                Ok((token, url, host_id)) => {
                    if bridged {
                        let title = dir.clone().unwrap_or_else(|| format!("127.0.0.1:{port}"));
                        self.bridge
                            .register(&token, &public_url(&url, explicit), &title, quiet);
                    }
                    record.preview_token = Some(token);
                    record.url = Some(public_url(&url, explicit));
                    record.host_id = Some(host_id);
                    record.tag = Some(tag);
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
        let call = |host: &Host| {
            self.hub.call(
                "POST",
                host::api(&self.hub, host, "previews"),
                Some(&host.secret),
                Payload::Json(body.clone()),
            )
        };
        let host = self.ensure_link()?;
        let answer = match call(&host) {
            Err(e) if e.recreates() => {
                self.drop_link();
                let fresh = self.store.replace(&self.hub, Some(&host))?;
                let host = {
                    let _guard = self.connecting.lock().unwrap();
                    self.connect(fresh)?
                };
                call(&host).map(|v| (v, host))
            }
            other => other.map(|v| (v, host)),
        };
        let (value, host) = answer?;
        let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
        match (text("previewToken"), text("url")) {
            (Some(token), Some(url)) => Ok((token, url, host.host_id)),
            _ => Err(HubError::Protocol {
                message: "the serve hub's mint answer has no previewToken or url".into(),
            }),
        }
    }

    fn ensure_link(&self) -> Result<Host, HubError> {
        let _guard = self.connecting.lock().unwrap();
        if let Some(link) = self.link.lock().unwrap().as_ref() {
            if link.alive.load(Ordering::SeqCst) {
                return Ok(link.host.clone());
            }
        }
        let (host, fresh) = self.store.current(&self.hub)?;
        match self.connect(host.clone()) {
            Err(e) if e.recreates() && !fresh => {
                let host = self.store.replace(&self.hub, Some(&host))?;
                self.connect(host)
            }
            other => other,
        }
    }

    fn drop_link(&self) {
        if let Some(link) = self.link.lock().unwrap().take() {
            link.alive.store(false, Ordering::SeqCst);
        }
    }

    fn connect(&self, host: Host) -> Result<Host, HubError> {
        let socket = ws::connect(&self.hub, &host::socket_url(&self.hub, &host), &host.secret)?;
        let (out, rx) = channel();
        let conn = Arc::new(Conn {
            out: out.clone(),
            streams: Mutex::new(HashMap::new()),
        });
        self.bridge.relinking();
        self.bridge.set_hub(out);
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
            host: host.clone(),
            alive: alive.clone(),
        });
        if let Some(previous) = previous {
            previous.alive.store(false, Ordering::SeqCst);
        }
        self.orphan(&host.host_id, "the serve hub dropped this machine's host");
        eprintln!("{NAME} relay: publishing for host {}", host.host_id);
        let me = self.self_arc();
        let h = host.clone();
        std::thread::spawn(move || me.pump(socket, rx, conn, alive, h, generation));
        let me = self.self_arc();
        let h = host.clone();
        std::thread::spawn(move || me.tidy(&h));
        Ok(host)
    }

    fn self_arc(&self) -> Arc<Daemon> {
        self.me.upgrade().expect("the daemon outlives its threads")
    }

    fn orphan(&self, current: &str, reason: &str) {
        let ids: Vec<String> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.record.host_id.as_deref().is_some_and(|h| h != current))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id, reason, false);
        }
    }

    fn tidy(&self, host: &Host) {
        let Ok(list) = self.hub.call(
            "GET",
            host::api(&self.hub, host, "previews"),
            Some(&host.secret),
            Payload::None,
        ) else {
            return;
        };
        let served: Vec<String> = self
            .entries
            .lock()
            .unwrap()
            .values()
            .filter_map(|e| e.origin.as_ref().map(|o| o.tag.clone()))
            .collect();
        for preview in list
            .get("previews")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let text = |k: &str| preview.get(k).and_then(Value::as_str).unwrap_or("");
            if text("mode") != "live" || served.iter().any(|s| s == text("tag")) {
                continue;
            }
            let token = text("previewToken");
            if token.is_empty() {
                continue;
            }
            eprintln!("{NAME} relay: stopping {token}, which nothing serves any more");
            let _ = self.hub.call(
                "DELETE",
                host::api(&self.hub, host, &format!("previews/{token}")),
                Some(&host.secret),
                Payload::None,
            );
        }
    }

    fn pump(
        self: Arc<Self>,
        mut socket: ws::Socket,
        rx: Receiver<Message>,
        conn: Arc<Conn>,
        alive: Arc<AtomicBool>,
        host: Host,
        generation: u64,
    ) {
        let mut pinged = Instant::now();
        let mut broken = false;
        let mut closed: Option<u16> = None;
        while alive.load(Ordering::SeqCst) && !broken {
            while let Ok(message) = rx.try_recv() {
                if socket.send(message).is_err() {
                    broken = true;
                    break;
                }
            }
            if pinged.elapsed() > PING {
                pinged = Instant::now();
                if socket.send(Message::text("ping")).is_err() {
                    broken = true;
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => self.dispatch(text.as_str(), &conn),
                Ok(Message::Binary(data)) => self.body(&data, &conn),
                Ok(Message::Close(frame)) => {
                    closed = frame.map(|f| u16::from(f.code));
                    broken = true;
                }
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
        conn.cancel_all();
        if !broken || self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        match closed {
            Some(4000) => {
                eprintln!(
                    "{NAME} relay: another socket took over host {}",
                    host.host_id
                );
                self.drop_link();
                self.stop_public("another scserve took over this machine's host");
            }
            Some(4001) => {
                eprintln!("{NAME} relay: the serve hub deleted host {}", host.host_id);
                self.drop_link();
                self.store.forget(&host.host_id);
                self.stop_public("the serve hub deleted this machine's host");
            }
            _ => {
                eprintln!("{NAME} relay: lost the serve hub, reconnecting");
                self.reconnect(host, generation);
            }
        }
    }

    fn public_entries(&self) -> bool {
        self.entries
            .lock()
            .unwrap()
            .values()
            .any(|e| e.origin.is_some())
    }

    fn stop_public(&self, reason: &str) {
        let ids: Vec<String> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, e)| e.origin.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.stop(&id, reason, false);
        }
    }

    fn reconnect(&self, host: Host, generation: u64) {
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
            match self.connect(host.clone()) {
                Ok(_) => return,
                Err(e) if e.recreates() => {
                    eprintln!("{NAME} relay: the serve hub dropped host {}", host.host_id);
                    self.drop_link();
                    self.stop_public("the serve hub dropped this machine's host");
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

    fn body(&self, data: &[u8], conn: &Conn) {
        let Some((id, bytes)) = relay::unframe(data) else {
            return;
        };
        let pushed = conn.stream(id).is_some_and(|s| s.push(bytes));
        if !pushed {
            conn.text(json!({ "type": "http.ack", "id": id, "bytes": bytes.len() }));
        }
    }

    fn request(&self, message: &Value, conn: &Arc<Conn>) {
        let Some(request) = relay::parse(message) else {
            return;
        };
        let (port, holder) = {
            let entries = self.entries.lock().unwrap();
            let found = entries
                .values()
                .find(|e| e.record.preview_token.as_deref() == Some(&request.preview_token));
            (
                found.and_then(|e| e.origin.as_ref().map(|o| o.port)),
                found.and_then(|e| e.holder.as_ref().and_then(|h| h.try_clone().ok())),
            )
        };
        let stream = Arc::new(Stream::new(request.body));
        conn.streams
            .lock()
            .unwrap()
            .insert(request.id, stream.clone());
        let conn = conn.clone();
        std::thread::spawn(move || {
            let outcome = relay::serve(&request, port, &stream, &conn.out);
            conn.streams.lock().unwrap().remove(&request.id);
            if let (Some(mut holder), true) = (holder, outcome.status > 0) {
                let log = format!(
                    "{} {} {} {}",
                    request.method, request.path, outcome.status, outcome.bytes
                );
                line(&mut holder, &json!({ "log": log }));
            }
        });
    }

    fn dispatch(&self, text: &str, conn: &Arc<Conn>) {
        let Ok(message) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let id = message
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok());
        let stream = || id.and_then(|i| conn.stream(i));
        match message.get("type").and_then(Value::as_str) {
            Some("http.request") => self.request(&message, conn),
            Some("http.request.end") => {
                if let Some(s) = stream() {
                    s.end();
                }
            }
            Some("http.ack") => {
                if let Some(s) = stream() {
                    s.ack(message.get("bytes").and_then(Value::as_u64).unwrap_or(0));
                }
            }
            Some("http.cancel") => {
                if let Some(s) = id.and_then(|i| conn.streams.lock().unwrap().remove(&i)) {
                    s.cancel();
                }
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
            Some("bridge.emit") => {
                if let Some(ev) = self.bridge.emitted(&message) {
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
                    self.stop(&id, "stopped on the serve hub", false);
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
        if let (true, Some(token), Some(host)) = (
            revoke,
            entry.record.preview_token.as_deref(),
            self.store
                .saved(&self.hub)
                .filter(|h| entry.record.host_id.as_deref() == Some(h.host_id.as_str())),
        ) {
            let _ = self.hub.call(
                "DELETE",
                host::api(&self.hub, &host, &format!("previews/{token}")),
                Some(&host.secret),
                Payload::None,
            );
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

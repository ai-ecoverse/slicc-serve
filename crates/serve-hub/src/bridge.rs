use crate::remote;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tungstenite::Message;

pub const RECORDS: usize = 500;
pub const DETAIL_CHARS: usize = 1000;
pub const TIMEOUT: Duration = Duration::from_secs(30);
const FRAME: &str = "preview-frame";
const LOADER: &str = "preview-loader";
const GONE: &str = "the visitor's tab is gone";
const LATE: &str = "the visitor's tab didn't answer within 30 s";
const NO_BROWSER: &str = "a preview bridge can't open or close the visitor's browser";

struct Preview {
    url: String,
    title: String,
    quiet: bool,
    announced: bool,
}

struct Tab {
    world: u64,
    stale: bool,
    token: String,
    origin: String,
    user_agent: String,
    url: String,
    title: String,
}

struct Session {
    conn: String,
    runtime: bool,
}

struct Client {
    tx: Sender<String>,
    discover: bool,
    auto: bool,
    sessions: HashMap<String, Session>,
}

struct Pending {
    client: u64,
    id: Value,
    session: String,
    conn: String,
    method: String,
    url: Option<String>,
    shape: Shape,
    group: Option<String>,
    at: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Raw,
    Remote,
    Properties,
    Describe,
    Resolve,
    Ignored,
}

#[derive(Default)]
struct State {
    hub: Option<Sender<Message>>,
    previews: HashMap<String, Preview>,
    tabs: BTreeMap<String, Tab>,
    clients: HashMap<u64, Client>,
    pending: HashMap<u64, Pending>,
    records: VecDeque<Value>,
    contexts: u64,
    groups: HashMap<(String, String), Vec<String>>,
    bridged: std::collections::HashSet<String>,
}

#[derive(Default)]
pub struct Bridge {
    state: Mutex<State>,
    next: AtomicU64,
    version: String,
}

fn text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn now() -> String {
    serve_core::time::iso(serve_core::time::now_ms())
}

fn target_id(tab: &Tab, conn: &str) -> String {
    format!("preview:{}:{conn}", tab.token)
}

fn info(tab: &Tab, conn: &str, attached: bool) -> Value {
    json!({
        "targetId": target_id(tab, conn),
        "type": "page",
        "title": tab.title,
        "url": tab.url,
        "attached": attached,
        "canAccessOpener": false,
        "browserContextId": "preview",
    })
}

fn event(method: &str, params: Value, session: Option<&str>) -> String {
    let mut m = json!({ "method": method, "params": params });
    if let Some(s) = session {
        m["sessionId"] = json!(s);
    }
    m.to_string()
}

fn answer(id: &Value, outcome: Result<Value, (i64, String)>, session: Option<&str>) -> String {
    let mut m = match outcome {
        Ok(result) => json!({ "id": id, "result": result }),
        Err((code, message)) => json!({ "id": id, "error": { "code": code, "message": message } }),
    };
    if let Some(s) = session {
        m["sessionId"] = json!(s);
    }
    m.to_string()
}

type Translated = ((String, Value), Shape);

fn evaluate(expression: String, params: &Value) -> (String, Value) {
    let mut sent = json!({ "expression": expression });
    for k in ["awaitPromise", "userGesture", "timeout"] {
        if let Some(v) = params.get(k) {
            sent[k] = v.clone();
        }
    }
    ("Runtime.evaluate".into(), sent)
}

fn translate(conn: &str, method: &str, params: &Value) -> Result<Translated, (i64, String)> {
    let by_value = params.get("returnByValue") == Some(&json!(true));
    let object = params.get("objectId").and_then(Value::as_str);
    let page = |id: &str| {
        remote::page_id(conn, id)
            .map(str::to_string)
            .ok_or((-32000, remote::GONE.to_string()))
    };
    Ok(match method {
        "Runtime.evaluate" => (
            evaluate(
                remote::evaluate(&text(params, "expression"), by_value),
                params,
            ),
            Shape::Remote,
        ),
        "Runtime.callFunctionOn" => {
            let this = object.map(page).transpose()?;
            let mut args = Vec::new();
            for arg in params
                .get("arguments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                args.push(remote::argument(conn, arg).ok_or((-32000, remote::GONE.to_string()))?);
            }
            let expression = remote::call(
                this.as_deref(),
                &text(params, "functionDeclaration"),
                &args,
                by_value,
            );
            (evaluate(expression, params), Shape::Remote)
        }
        "Runtime.getProperties" => {
            let id = page(object.unwrap_or(""))?;
            let accessors = params.get("accessorPropertiesOnly") == Some(&json!(true));
            (
                evaluate(remote::properties(&id, accessors), params),
                Shape::Properties,
            )
        }
        "DOM.describeNode" => {
            let backend = params.get("backendNodeId").and_then(Value::as_u64);
            let id = object.map(page).transpose()?;
            if id.is_none() && backend.is_none() {
                return Err((
                    -32000,
                    "the preview bridge has no DOM node ids; describe a node by its objectId"
                        .into(),
                ));
            }
            (
                evaluate(remote::describe(id.as_deref(), backend), params),
                Shape::Describe,
            )
        }
        "DOM.resolveNode" => {
            let Some(backend) = params.get("backendNodeId").and_then(Value::as_u64) else {
                return Err((
                    -32000,
                    "the preview bridge resolves nodes by backendNodeId only".into(),
                ));
            };
            (evaluate(remote::resolve(backend), params), Shape::Resolve)
        }
        _ if object.is_some_and(remote::is_handle) => {
            return Err((
                -32000,
                format!("the preview bridge can't pass a page object to {method}"),
            ))
        }
        _ => ((method.to_string(), params.clone()), Shape::Raw),
    })
}

fn cut(detail: &Value) -> String {
    let text = detail.to_string();
    if text.chars().count() > DETAIL_CHARS {
        format!("{}…", text.chars().take(DETAIL_CHARS).collect::<String>())
    } else {
        text
    }
}

impl State {
    fn record(&mut self, record: Value) {
        if self.records.len() >= RECORDS {
            self.records.pop_front();
        }
        self.records.push_back(record);
    }

    fn to_hub(&self, message: Value) -> bool {
        self.hub
            .as_ref()
            .is_some_and(|h| h.send(Message::text(message.to_string())).is_ok())
    }

    fn attach(&mut self, client: u64, conn: &str, id: String) -> Option<String> {
        let tab = self.tabs.get(conn)?;
        let targ = info(tab, conn, true);
        let c = self.clients.get_mut(&client)?;
        c.sessions.insert(
            id.clone(),
            Session {
                conn: conn.to_string(),
                runtime: false,
            },
        );
        let _ = c.tx.send(event(
            "Target.attachedToTarget",
            json!({ "sessionId": id, "targetInfo": targ, "waitingForDebugger": false }),
            None,
        ));
        Some(id)
    }

    fn drop_tab(&mut self, conn: &str) -> Option<Tab> {
        let tab = self.tabs.remove(conn)?;
        self.groups.retain(|(c, _), _| c != conn);
        let target = target_id(&tab, conn);
        for c in self.clients.values_mut() {
            let ended: Vec<String> = c
                .sessions
                .iter()
                .filter(|(_, s)| s.conn == conn)
                .map(|(id, _)| id.clone())
                .collect();
            for id in ended {
                c.sessions.remove(&id);
                let _ = c.tx.send(event(
                    "Target.detachedFromTarget",
                    json!({ "sessionId": id, "targetId": target }),
                    None,
                ));
            }
            if c.discover {
                let _ = c.tx.send(event(
                    "Target.targetDestroyed",
                    json!({ "targetId": target }),
                    None,
                ));
            }
        }
        let failed: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| p.conn == conn)
            .map(|(id, _)| *id)
            .collect();
        for id in failed {
            self.fail(id, GONE);
        }
        Some(tab)
    }

    fn fail(&mut self, id: u64, message: &str) {
        if let Some(p) = self.pending.remove(&id) {
            if let Some(c) = self.clients.get(&p.client) {
                let _ = c.tx.send(answer(
                    &p.id,
                    Err((-32000, message.to_string())),
                    Some(&p.session),
                ));
            }
        }
    }

    fn announce(&self, client: u64, session: &str) {
        let Some(c) = self.clients.get(&client) else {
            return;
        };
        let Some(tab) = c.sessions.get(session).and_then(|s| self.tabs.get(&s.conn)) else {
            return;
        };
        let id = tab.world;
        let _ = c.tx.send(event(
            "Runtime.executionContextCreated",
            json!({ "context": {
                "id": id,
                "origin": tab.origin,
                "name": "",
                "uniqueId": format!("preview-{id}"),
                "auxData": { "isDefault": true, "type": "default", "frameId": FRAME },
            } }),
            Some(session),
        ));
    }

    fn enable_runtime(&mut self, client: u64, session: &str) {
        let Some(s) = self
            .clients
            .get_mut(&client)
            .and_then(|c| c.sessions.get_mut(session))
        else {
            return;
        };
        if s.runtime {
            return;
        }
        s.runtime = true;
        self.announce(client, session);
    }

    fn world_of(&self, client: u64, session: &str) -> u64 {
        self.clients
            .get(&client)
            .and_then(|c| c.sessions.get(session))
            .and_then(|s| self.tabs.get(&s.conn))
            .map(|t| t.world)
            .unwrap_or(0)
    }

    fn navigated(&mut self, conn: &str) {
        self.groups.retain(|(c, _), _| c != conn);
        self.contexts += 1;
        let id = self.contexts;
        if let Some(tab) = self.tabs.get_mut(conn) {
            tab.world = id;
        }
        let listeners: Vec<(u64, String)> = self
            .clients
            .iter()
            .flat_map(|(cid, c)| {
                c.sessions
                    .iter()
                    .filter(|(_, s)| s.conn == conn && s.runtime)
                    .map(move |(sid, _)| (*cid, sid.clone()))
            })
            .collect();
        for (client, session) in listeners {
            if let Some(c) = self.clients.get(&client) {
                let _ = c.tx.send(event(
                    "Runtime.executionContextsCleared",
                    json!({}),
                    Some(&session),
                ));
            }
            self.announce(client, &session);
        }
    }
}

impl Bridge {
    pub fn new(version: &str) -> Bridge {
        Bridge {
            version: version.to_string(),
            ..Bridge::default()
        }
    }

    pub fn set_hub(&self, hub: Sender<Message>) {
        self.state.lock().unwrap().hub = Some(hub);
    }

    pub fn register(&self, token: &str, url: &str, title: &str, quiet: bool) {
        self.state.lock().unwrap().bridged.insert(token.to_string());
        self.state.lock().unwrap().previews.insert(
            token.to_string(),
            Preview {
                url: url.to_string(),
                title: title.to_string(),
                quiet,
                announced: false,
            },
        );
    }

    pub fn unregister(&self, token: &str) {
        let mut st = self.state.lock().unwrap();
        st.previews.remove(token);
        let conns: Vec<String> = st
            .tabs
            .iter()
            .filter(|(_, t)| t.token == token)
            .map(|(c, _)| c.clone())
            .collect();
        for conn in conns {
            st.drop_tab(&conn);
        }
    }

    pub fn known(&self, token: &str) -> bool {
        let st = self.state.lock().unwrap();
        st.bridged.contains(token)
    }

    pub fn version_info(&self, ws: &str) -> Value {
        json!({
            "Browser": format!("scserve/{}", self.version),
            "Protocol-Version": "1.3",
            "User-Agent": format!("scserve/{}", self.version),
            "webSocketDebuggerUrl": ws,
        })
    }

    pub fn list(&self) -> Value {
        let st = self.state.lock().unwrap();
        Value::Array(
            st.tabs
                .iter()
                .map(|(conn, t)| {
                    json!({
                        "id": target_id(t, conn),
                        "type": "page",
                        "title": t.title,
                        "url": t.url,
                        "description": format!("{} {}", t.origin, t.user_agent),
                    })
                })
                .collect(),
        )
    }

    pub fn preview_state(&self, message: &Value) {
        let mut st = self.state.lock().unwrap();
        if let Some(p) = st.previews.get_mut(&text(message, "previewToken")) {
            if let Some(q) = message.get("quiet").and_then(Value::as_bool) {
                p.quiet = q;
            }
            if let Some(a) = message.get("announced").and_then(Value::as_bool) {
                p.announced = a;
            }
        }
    }

    pub fn connected(&self, message: &Value) -> Option<Value> {
        let token = text(message, "previewToken");
        let conn = text(message, "connId");
        if conn.is_empty() {
            return None;
        }
        let replay = message.get("replay") == Some(&json!(true));
        let mut st = self.state.lock().unwrap();
        if let Some(tab) = st.tabs.get_mut(&conn) {
            tab.stale = false;
            return None;
        }
        let (url, title, first, quiet) = {
            let p = st.previews.get_mut(&token)?;
            let first = !p.announced && !replay;
            if first || replay {
                p.announced = true;
            }
            (p.url.clone(), p.title.clone(), first, p.quiet)
        };
        st.contexts += 1;
        let tab = Tab {
            world: st.contexts,
            stale: false,
            token: token.clone(),
            origin: text(message, "origin"),
            user_agent: text(message, "userAgent"),
            url,
            title,
        };
        let created = info(&tab, &conn, false);
        st.tabs.insert(conn.clone(), tab);
        let clients: Vec<u64> = st.clients.keys().copied().collect();
        for client in clients {
            let (discover, auto) = {
                let c = &st.clients[&client];
                (c.discover, c.auto)
            };
            if discover {
                let _ = st.clients[&client].tx.send(event(
                    "Target.targetCreated",
                    json!({ "targetInfo": created }),
                    None,
                ));
            }
            if auto {
                let id = self.session_id();
                st.attach(client, &conn, id);
            }
        }
        if replay {
            return None;
        }
        if first {
            st.to_hub(
                json!({ "type": "preview.state.update", "previewToken": token, "announced": true }),
            );
        }
        let record = json!({
            "at": now(),
            "event": "connected",
            "previewToken": token,
            "connId": conn,
            "announced": first && !quiet,
            "origin": text(message, "origin"),
            "userAgent": text(message, "userAgent"),
            "connectedAt": text(message, "connectedAt"),
        });
        st.record(record.clone());
        Some(record)
    }

    pub fn disconnected(&self, message: &Value) -> Option<Value> {
        let conn = text(message, "connId");
        let mut st = self.state.lock().unwrap();
        let tab = st.drop_tab(&conn)?;
        let mut record = json!({
            "at": now(),
            "event": "disconnected",
            "previewToken": tab.token,
            "connId": conn,
            "announced": false,
            "origin": tab.origin,
            "userAgent": tab.user_agent,
        });
        if let Some(reason) = message.get("reason").and_then(Value::as_str) {
            record["reason"] = json!(reason);
        }
        st.record(record.clone());
        Some(record)
    }

    pub fn emitted(&self, message: &Value) -> Option<Value> {
        let token = text(message, "previewToken");
        let conn = message.get("connId").and_then(Value::as_str)?.to_string();
        let mut st = self.state.lock().unwrap();
        let live = st.tabs.get(&conn).is_some_and(|t| t.token == token);
        if !st.previews.contains_key(&token) || !live {
            return None;
        }
        let body = message;
        let name = match body.get("name") {
            Some(Value::String(s)) => s.chars().take(200).collect::<String>(),
            Some(other) => cut(other),
            None => String::new(),
        };
        let record = json!({
            "at": now(),
            "event": "emit",
            "previewToken": token,
            "connId": conn,
            "announced": false,
            "name": name,
            "detail": cut(body.get("detail").unwrap_or(&Value::Null)),
        });
        st.record(record.clone());
        Some(record)
    }

    pub fn relinking(&self) {
        for tab in self.state.lock().unwrap().tabs.values_mut() {
            tab.stale = true;
        }
    }

    pub fn drop_stale(&self) -> Vec<Value> {
        let mut st = self.state.lock().unwrap();
        let stale: Vec<String> = st
            .tabs
            .iter()
            .filter(|(_, t)| t.stale)
            .map(|(c, _)| c.clone())
            .collect();
        let mut gone = Vec::new();
        for conn in stale {
            let Some(tab) = st.drop_tab(&conn) else {
                continue;
            };
            let record = json!({
                "at": now(),
                "event": "disconnected",
                "previewToken": tab.token,
                "connId": conn,
                "announced": false,
                "origin": tab.origin,
                "userAgent": tab.user_agent,
                "reason": "the serve hub no longer had it after the relay reconnected",
            });
            st.record(record.clone());
            gone.push(record);
        }
        gone
    }

    pub fn responded(&self, message: &Value) {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return;
        };
        let mut st = self.state.lock().unwrap();
        let Some(p) = st.pending.remove(&id) else {
            return;
        };
        let outcome = match message.get("error") {
            Some(e) if !e.is_null() => Err((
                e.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                text(e, "message"),
            )),
            _ => Ok(message.get("result").cloned().unwrap_or_else(|| json!({}))),
        };
        let outcome = match p.shape {
            Shape::Raw => outcome,
            Shape::Ignored => return,
            Shape::Remote => outcome.and_then(|r| remote::result(&p.conn, r)),
            Shape::Properties => outcome.and_then(|r| remote::listed(&p.conn, r)),
            Shape::Describe => outcome.and_then(remote::described),
            Shape::Resolve => outcome.and_then(|r| remote::resolved(&p.conn, r)),
        };
        if let (Some(group), Ok(r)) = (&p.group, &outcome) {
            if let Some(page) = r
                .pointer("/result/objectId")
                .or_else(|| r.pointer("/object/objectId"))
                .and_then(Value::as_str)
                .and_then(|id| remote::page_id(&p.conn, id))
            {
                st.groups
                    .entry((p.conn.clone(), group.clone()))
                    .or_default()
                    .push(page.to_string());
            }
        }
        let navigated = p.method == "Page.navigate" && outcome.is_ok();
        let frame = FRAME;
        let outcome = if navigated {
            outcome.map(|_| json!({ "frameId": frame, "loaderId": LOADER }))
        } else {
            outcome
        };
        if let Some(c) = st.clients.get(&p.client) {
            let _ = c.tx.send(answer(&p.id, outcome, Some(&p.session)));
        }
        if !navigated {
            return;
        }
        let url = match p.url {
            Some(u) => {
                if let Some(t) = st.tabs.get_mut(&p.conn) {
                    t.url = u.clone();
                }
                u
            }
            None => st
                .tabs
                .get(&p.conn)
                .map(|t| t.url.clone())
                .unwrap_or_default(),
        };
        let origin = st
            .tabs
            .get(&p.conn)
            .map(|t| t.origin.clone())
            .unwrap_or_default();
        let sessions: Vec<(Sender<String>, String)> = st
            .clients
            .values()
            .flat_map(|c| {
                c.sessions
                    .iter()
                    .filter(|(_, s)| s.conn == p.conn)
                    .map(|(sid, _)| (c.tx.clone(), sid.clone()))
            })
            .collect();
        for (tx, sid) in &sessions {
            let _ = tx.send(event(
                "Page.frameNavigated",
                json!({ "frame": { "id": frame, "loaderId": LOADER, "url": url, "securityOrigin": origin, "mimeType": "text/html" }, "type": "Navigation" }),
                Some(sid),
            ));
        }
        st.navigated(&p.conn);
        for (tx, sid) in &sessions {
            let _ = tx.send(event(
                "Page.loadEventFired",
                json!({ "timestamp": serve_core::time::now_ms() as f64 / 1000.0 }),
                Some(sid),
            ));
        }
    }

    pub fn expire(&self) {
        let mut st = self.state.lock().unwrap();
        let late: Vec<u64> = st
            .pending
            .iter()
            .filter(|(_, p)| p.at.elapsed() >= TIMEOUT)
            .map(|(id, _)| *id)
            .collect();
        for id in late {
            st.fail(id, LATE);
        }
    }

    pub fn logs(&self, token: Option<&str>) -> Vec<Value> {
        let st = self.state.lock().unwrap();
        st.records
            .iter()
            .filter(|r| token.is_none() || r.get("previewToken").and_then(Value::as_str) == token)
            .cloned()
            .collect()
    }

    pub fn truncate(&self, token: Option<&str>) -> (usize, usize) {
        let mut st = self.state.lock().unwrap();
        let before = st.records.len();
        st.records
            .retain(|r| token.is_some() && r.get("previewToken").and_then(Value::as_str) != token);
        let cleared = before - st.records.len();
        let tokens: Vec<String> = st
            .previews
            .keys()
            .filter(|t| token.is_none() || token == Some(t.as_str()))
            .cloned()
            .collect();
        for t in &tokens {
            if let Some(p) = st.previews.get_mut(t) {
                p.announced = false;
            }
            st.to_hub(
                json!({ "type": "preview.state.update", "previewToken": t, "announced": false }),
            );
        }
        (cleared, tokens.len())
    }

    fn session_id(&self) -> String {
        format!(
            "{:032X}",
            self.next.fetch_add(1, Ordering::SeqCst) as u128
                + 1
                + ((std::process::id() as u128) << 64)
        )
    }

    pub fn open(&self, tx: Sender<String>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.state.lock().unwrap().clients.insert(
            id,
            Client {
                tx,
                discover: false,
                auto: false,
                sessions: HashMap::new(),
            },
        );
        id
    }

    pub fn close(&self, client: u64) {
        let mut st = self.state.lock().unwrap();
        st.clients.remove(&client);
        st.pending.retain(|_, p| p.client != client);
    }

    pub fn message(&self, client: u64, raw: &str) {
        let Ok(m) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let id = m.get("id").cloned().unwrap_or(Value::Null);
        let method = text(&m, "method");
        let params = m.get("params").cloned().unwrap_or_else(|| json!({}));
        let session = m
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let outcome = match &session {
            Some(s) => self.in_session(client, &id, s, &method, params),
            None => self.browser(client, &method, &params),
        };
        if let Some(outcome) = outcome {
            let st = self.state.lock().unwrap();
            if let Some(c) = st.clients.get(&client) {
                let _ = c.tx.send(answer(&id, outcome, session.as_deref()));
            }
        }
    }

    fn browser(
        &self,
        client: u64,
        method: &str,
        params: &Value,
    ) -> Option<Result<Value, (i64, String)>> {
        let mut st = self.state.lock().unwrap();
        let attached = |st: &State, conn: &str| {
            st.clients
                .get(&client)
                .is_some_and(|c| c.sessions.values().any(|s| s.conn == conn))
        };
        Some(match method {
            "Browser.getVersion" => Ok(json!({
                "protocolVersion": "1.3",
                "product": format!("scserve/{}", self.version),
                "revision": "",
                "userAgent": format!("scserve/{}", self.version),
                "jsVersion": "",
            })),
            "Target.getTargets" => Ok(json!({ "targetInfos": st
                .tabs
                .iter()
                .map(|(conn, t)| info(t, conn, attached(&st, conn)))
                .collect::<Vec<_>>() })),
            "Target.setDiscoverTargets" => {
                let on = params
                    .get("discover")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let created: Vec<Value> = st
                    .tabs
                    .iter()
                    .map(|(conn, t)| info(t, conn, attached(&st, conn)))
                    .collect();
                if let Some(c) = st.clients.get_mut(&client) {
                    let fresh = on && !c.discover;
                    c.discover = on;
                    if fresh {
                        for t in created {
                            let _ = c.tx.send(event(
                                "Target.targetCreated",
                                json!({ "targetInfo": t }),
                                None,
                            ));
                        }
                    }
                }
                Ok(json!({}))
            }
            "Target.setAutoAttach" => {
                let on = params
                    .get("autoAttach")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let conns: Vec<String> = st
                    .tabs
                    .keys()
                    .filter(|conn| !attached(&st, conn))
                    .cloned()
                    .collect();
                if let Some(c) = st.clients.get_mut(&client) {
                    c.auto = on;
                }
                if on {
                    for conn in conns {
                        let id = self.session_id();
                        st.attach(client, &conn, id);
                    }
                }
                Ok(json!({}))
            }
            "Target.attachToTarget" => {
                let target = text(params, "targetId");
                let conn = st
                    .tabs
                    .iter()
                    .find(|(conn, t)| target_id(t, conn) == target)
                    .map(|(conn, _)| conn.clone());
                match conn {
                    Some(conn) => {
                        let id = self.session_id();
                        match st.attach(client, &conn, id) {
                            Some(id) => Ok(json!({ "sessionId": id })),
                            None => Err((-32602, "No target with given id found".into())),
                        }
                    }
                    None => Err((-32602, "No target with given id found".into())),
                }
            }
            "Target.detachFromTarget" => {
                let sid = text(params, "sessionId");
                let ended = st
                    .clients
                    .get_mut(&client)
                    .and_then(|c| c.sessions.remove(&sid).map(|s| (s, c.tx.clone())));
                match ended {
                    Some((s, tx)) => {
                        let target = st
                            .tabs
                            .get(&s.conn)
                            .map(|t| target_id(t, &s.conn))
                            .unwrap_or_default();
                        let _ = tx.send(event(
                            "Target.detachedFromTarget",
                            json!({ "sessionId": sid, "targetId": target }),
                            None,
                        ));
                        Ok(json!({}))
                    }
                    None => Err((-32602, "No session with given id".into())),
                }
            }
            "Target.closeTarget" => {
                let target = text(params, "targetId");
                let conn = st
                    .tabs
                    .iter()
                    .find(|(conn, t)| target_id(t, conn) == target)
                    .map(|(conn, _)| conn.clone());
                match conn {
                    Some(conn) => {
                        st.to_hub(json!({ "type": "bridge.close", "connId": conn }));
                        Ok(json!({ "success": true }))
                    }
                    None => Err((-32602, "No target with given id found".into())),
                }
            }
            "Target.getTargetInfo" => {
                let target = text(params, "targetId");
                match st
                    .tabs
                    .iter()
                    .find(|(conn, t)| target.is_empty() || target_id(t, conn) == target)
                {
                    Some((conn, t)) if !target.is_empty() => {
                        Ok(json!({ "targetInfo": info(t, conn, attached(&st, conn)) }))
                    }
                    _ if target.is_empty() => Ok(json!({ "targetInfo": {
                        "targetId": "browser", "type": "browser", "title": "", "url": "", "attached": true,
                    } })),
                    _ => Err((-32602, "No target with given id found".into())),
                }
            }
            "Target.getBrowserContexts" => Ok(json!({ "browserContextIds": [] })),
            "Target.activateTarget" => Ok(json!({})),
            "Target.createTarget" | "Target.createBrowserContext" | "Browser.close" => {
                Err((-32000, NO_BROWSER.into()))
            }
            _ => Err((-32601, format!("'{method}' wasn't found"))),
        })
    }

    fn in_session(
        &self,
        client: u64,
        id: &Value,
        session: &str,
        method: &str,
        params: Value,
    ) -> Option<Result<Value, (i64, String)>> {
        let mut st = self.state.lock().unwrap();
        let Some(conn) = st
            .clients
            .get(&client)
            .and_then(|c| c.sessions.get(session))
            .map(|s| s.conn.clone())
        else {
            return Some(Err((
                -32001,
                format!("Session with given id not found: {session}"),
            )));
        };
        let (url, origin, frame) = match st.tabs.get(&conn) {
            Some(t) => (t.url.clone(), t.origin.clone(), FRAME),
            None => return Some(Err((-32000, GONE.into()))),
        };
        let set_runtime = |st: &mut State, on: bool| {
            if let Some(s) = st
                .clients
                .get_mut(&client)
                .and_then(|c| c.sessions.get_mut(session))
            {
                let was = s.runtime;
                s.runtime = on;
                return was;
            }
            on
        };
        match method {
            "Page.enable"
            | "DOM.enable"
            | "Page.bringToFront"
            | "Runtime.runIfWaitingForDebugger"
            | "Target.setAutoAttach"
            | "Inspector.enable"
            | "Fetch.enable"
            | "Fetch.disable"
            | "Network.enable"
            | "Network.disable"
            | "Network.setBypassServiceWorker"
            | "Input.dispatchMouseEvent"
            | "Input.dispatchKeyEvent"
            | "Input.insertText" => Some(Ok(json!({}))),
            "Network.getAllCookies" | "Network.getCookies" | "Storage.getCookies" => {
                Some(Ok(json!({ "cookies": [] })))
            }
            "Runtime.disable" => {
                set_runtime(&mut st, false);
                Some(Ok(json!({})))
            }
            "Runtime.enable" => {
                st.enable_runtime(client, session);
                Some(Ok(json!({})))
            }
            "Page.getFrameTree" => Some(Ok(json!({ "frameTree": {
                "frame": { "id": frame, "loaderId": LOADER, "url": url, "securityOrigin": origin, "mimeType": "text/html" },
                "childFrames": [],
            } }))),
            "Page.createIsolatedWorld" => Some(Ok(
                json!({ "executionContextId": st.world_of(client, session) }),
            )),
            "Runtime.releaseObject" => {
                if let Some(page) = remote::page_id(&conn, &text(&params, "objectId")) {
                    let pages = vec![page.to_string()];
                    self.fire(&mut st, &conn, session, remote::release(&pages));
                }
                Some(Ok(json!({})))
            }
            "Runtime.releaseObjectGroup" => {
                let group = text(&params, "objectGroup");
                if let Some(pages) = st.groups.remove(&(conn.clone(), group)) {
                    self.fire(&mut st, &conn, session, remote::release(&pages));
                }
                Some(Ok(json!({})))
            }
            "Target.getTargetInfo" => {
                let tab = st.tabs.get(&conn)?;
                Some(Ok(json!({ "targetInfo": info(tab, &conn, true) })))
            }
            "Target.closeTarget" => {
                st.to_hub(json!({ "type": "bridge.close", "connId": conn }));
                Some(Ok(json!({ "success": true })))
            }
            _ => {
                let (sent, shape) = match translate(&conn, method, &params) {
                    Ok(t) => t,
                    Err(e) => return Some(Err(e)),
                };
                let group = params
                    .get("objectGroup")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let relay = self.next.fetch_add(1, Ordering::SeqCst) + 1;
                let (sent_method, sent_params) = sent;
                if !self.forward(&st, &conn, session, relay, &sent_method, sent_params) {
                    return Some(Err((
                        -32000,
                        "the relay isn't connected to the serve hub".into(),
                    )));
                }
                st.pending.insert(
                    relay,
                    Pending {
                        client,
                        id: id.clone(),
                        session: session.to_string(),
                        conn,
                        method: method.to_string(),
                        url: params
                            .get("url")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        shape,
                        group,
                        at: Instant::now(),
                    },
                );
                None
            }
        }
    }

    fn forward(
        &self,
        st: &State,
        conn: &str,
        session: &str,
        relay: u64,
        method: &str,
        params: Value,
    ) -> bool {
        let mut request = Map::new();
        request.insert("type".into(), json!("bridge.cdp.request"));
        request.insert("connId".into(), json!(conn));
        request.insert("id".into(), json!(relay));
        request.insert("method".into(), json!(method));
        request.insert("params".into(), params);
        request.insert("sessionId".into(), json!(session));
        st.to_hub(Value::Object(request))
    }

    fn fire(&self, st: &mut State, conn: &str, session: &str, expression: String) {
        let relay = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        if self.forward(
            st,
            conn,
            session,
            relay,
            "Runtime.evaluate",
            json!({ "expression": expression }),
        ) {
            st.pending.insert(
                relay,
                Pending {
                    client: 0,
                    id: Value::Null,
                    session: session.to_string(),
                    conn: conn.to_string(),
                    method: "Runtime.evaluate".into(),
                    url: None,
                    shape: Shape::Ignored,
                    group: None,
                    at: Instant::now(),
                },
            );
        }
    }
}

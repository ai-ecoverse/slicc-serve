use super::embedder;
use super::out::{self, Failure};
use super::store::{random_hex, Store};
use super::sys::{self, Op};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use serve_core::args::Serve;
use serve_core::record::{public_url, Record};
use serve_core::server::{self, Site};
use serve_core::time;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const ARG: &str = "__hold";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Spec {
    pub dir: Option<String>,
    pub port: Option<u16>,
    pub entry: Option<String>,
    pub local: bool,
    pub foreground: bool,
    #[serde(default)]
    pub json: bool,
    #[serde(default)]
    pub bridge: bool,
    #[serde(default)]
    pub max_tabs: Option<u32>,
    #[serde(default)]
    pub quiet: bool,
}

struct Held {
    record: Record,
    reader: Option<BufReader<File>>,
    server: Option<std::thread::JoinHandle<()>>,
}

fn serve_dir(
    cmd: &str,
    dir: &str,
    logging: bool,
) -> Result<(u16, std::thread::JoinHandle<()>), Failure> {
    let site = Site::new(std::path::Path::new(dir))
        .map_err(|e| Failure::plain(format!("{cmd}: {dir}: {e}")))?;
    let listener = wasix_net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Failure::plain(format!("{cmd}: can't listen on 127.0.0.1: {e}")))?;
    let port = listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| Failure::plain(format!("{cmd}: {e}")))?;
    let name = cmd.to_string();
    let log: server::Log = Arc::new(move |req, reply| {
        if logging {
            eprintln!(
                "{name}: {} {} {} {}",
                req.method,
                req.target,
                reply.status,
                reply.len()
            );
        }
    });
    let site = Arc::new(site);
    let handle = std::thread::spawn(move || {
        server::serve(listener, site, Arc::new(AtomicBool::new(false)), log)
    });
    Ok((port, handle))
}

fn start(cmd: &str, spec: &Spec, store: &Store) -> Result<Held, Failure> {
    let (port, server) = match (&spec.dir, spec.port) {
        (Some(dir), _) => {
            let (port, handle) = serve_dir(cmd, dir, spec.foreground)?;
            (port, Some(handle))
        }
        (None, Some(port)) => (port, None),
        (None, None) => return Err(Failure::plain(format!("{cmd}: no directory and no port"))),
    };
    let entry = spec.entry.clone();
    let mut record = Record {
        id: random_hex(8),
        mode: if spec.local { "local" } else { "live" }.into(),
        local_url: Some(format!(
            "{}{}",
            out::page_url(port),
            entry.as_deref().unwrap_or("")
        )),
        root: spec.dir.clone(),
        port: Some(port),
        entry: entry.clone(),
        created_at: time::iso(time::now_ms()),
        pid: Some(sys::pid()),
        foreground: spec.foreground,
        ..Record::default()
    };
    let mut reader = None;
    if !spec.local {
        let mut request = json!({ "kind": "serve.publish", "port": port });
        if let Some(dir) = &spec.dir {
            request["root"] = json!(dir);
        }
        if let Some(entry) = &entry {
            request["entry"] = json!(entry);
        }
        if spec.bridge {
            request["bridge"] = json!(true);
            request["maxTabs"] = json!(spec.max_tabs.unwrap_or(20));
            if spec.quiet {
                request["quiet"] = json!(true);
            }
        }
        let (held, line) = embedder::publish(&request).map_err(|e| Failure::page(cmd, &e))?;
        let answer: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
        if line.trim().is_empty() {
            return Err(Failure::plain(format!(
                "{cmd}: seven ended the request without a URL"
            )));
        }
        if answer.get("error").is_some() {
            return Err(Failure::hub(cmd, &answer, "publishing"));
        }
        let text = |k: &str| answer.get(k).and_then(Value::as_str).map(str::to_string);
        let (Some(url), Some(token)) = (text("url"), text("previewToken")) else {
            return Err(Failure::plain(format!(
                "{cmd}: seven's answer to serve.publish has no url or previewToken"
            )));
        };
        record.url = Some(public_url(&url, entry.is_some()));
        record.preview_token = Some(token);
        record.host_id = text("hostId");
        if spec.bridge {
            if text("webhook").is_none() {
                return Err(Failure::plain(format!(
                    "{cmd}: this seven publishes without the bridge, so {cmd} stopped the preview again; --bridge needs a newer slicc-bios"
                )));
            }
            record.bridge = true;
            record.cdp_url = std::env::var("SLICC_CDP_URL")
                .ok()
                .filter(|u| !u.is_empty());
        }
        reader = Some(held);
    }
    store
        .save(&record)
        .map_err(|e| Failure::plain(format!("{cmd}: can't save the record: {e}")))?;
    Ok(Held {
        record,
        reader,
        server,
    })
}

fn wait(cmd: &str, held: Held, store: &Store, json: bool) -> i32 {
    let Held {
        record,
        reader,
        server,
    } = held;
    let Some(mut reader) = reader else {
        if let Some(server) = server {
            let _ = server.join();
        }
        store.remove(&record.id);
        return 0;
    };
    let mut reason = None;
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(_) => {
                store.remove(&record.id);
                std::process::exit(1)
            }
            Ok(_) => {
                let value: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                if let Some(r) = value.get("stopped").and_then(Value::as_str) {
                    reason = Some(r.to_string());
                }
                if record.foreground && value.get("event").and_then(Value::as_str).is_some() {
                    for l in serve_core::table::event_line(cmd, &value, record.shown_url()) {
                        eprintln!("{l}");
                    }
                    if json {
                        println!("{value}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                }
            }
        }
    }
    store.remove(&record.id);
    if record.foreground {
        eprintln!(
            "{cmd}: stopped: {}",
            reason.as_deref().unwrap_or("stopped in seven")
        );
    }
    let _ = std::io::stderr().flush();
    std::process::exit(0)
}

pub fn foreground(cmd: &str, spec: &Spec, opts: &Serve) -> i32 {
    let store = Store::new();
    match start(cmd, spec, &store) {
        Ok(held) => {
            out::done(cmd, &held.record, opts);
            wait(cmd, held, &store, spec.json)
        }
        Err(f) => out::fail(&f, opts.json),
    }
}

fn report(value: &Value) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{value}");
    let _ = stdout.flush();
    if let Ok(null) = std::fs::OpenOptions::new().write(true).open("/dev/null") {
        use std::os::fd::IntoRawFd;
        let _ = sys::renumber(null.into_raw_fd() as u32, 1);
    }
}

pub fn background(cmd: &str, raw: &str) -> i32 {
    if let Err(e) = sys::ignore(sys::SIGHUP) {
        eprintln!("{cmd}: can't ignore SIGHUP, so a closing terminal may stop this preview: {e}");
    }
    let Ok(spec) = serde_json::from_str::<Spec>(raw) else {
        report(&Failure::plain(format!("{cmd}: a bad request for the background {cmd}")).to_json());
        return 2;
    };
    let store = Store::new();
    match start(cmd, &spec, &store) {
        Ok(held) => {
            report(&json!({ "ok": true, "record": held.record }));
            eprintln!(
                "{cmd}: {} {} on 127.0.0.1:{}",
                held.record.id,
                held.record.shown_url(),
                held.record.port.unwrap_or(0)
            );
            wait(cmd, held, &store, spec.json)
        }
        Err(f) => {
            report(&f.to_json());
            f.exit.code()
        }
    }
}

pub fn detach(cmd: &str, spec: &Spec, opts: &Serve) -> i32 {
    let store = Store::new();
    if let Err(e) = store.ensure() {
        let line = format!("{cmd}: can't create {}: {e}", store.state.display());
        return out::fail(&Failure::plain(line), opts.json);
    }
    let (r, w) = match sys::pipe() {
        Ok(p) => p,
        Err(e) => return out::fail(&Failure::plain(format!("{cmd}: {e}")), opts.json),
    };
    let argv0 = std::env::args().next().unwrap_or_else(|| cmd.to_string());
    let argv = vec![
        argv0,
        ARG.to_string(),
        serde_json::to_string(spec).unwrap_or_default(),
    ];
    let ops = [
        Op::Null(0),
        Op::Dup2 { src: w, fd: 1 },
        Op::Close(w),
        Op::Close(r),
        Op::Append {
            fd: 2,
            path: store.log().to_string_lossy().into_owned(),
        },
    ];
    let spawned = sys::spawn(&argv, &ops);
    let _ = sys::close(w);
    if let Err(e) = spawned {
        let _ = sys::close(r);
        let line = format!("{cmd}: can't start the background {cmd}: {e}");
        return out::fail(&Failure::plain(line), opts.json);
    }
    let mut text = String::new();
    let _ = sys::file(r).read_to_string(&mut text);
    let answer: Value = text
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or(Value::Null);
    if answer.get("ok") == Some(&json!(true)) {
        let record: Record = serde_json::from_value(answer["record"].clone()).unwrap_or_default();
        return out::done(cmd, &record, opts);
    }
    if answer.get("ok") == Some(&json!(false)) {
        return out::fail(&Failure::from_json(&answer), opts.json);
    }
    let line = format!(
        "{cmd}: the background {cmd} ended before it answered; see {}",
        store.log().display()
    );
    out::fail(&Failure::plain(line), opts.json)
}

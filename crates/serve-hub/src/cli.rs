use crate::client;
use crate::daemon::{self, Failure};
use crate::hub::Hub;
use crate::list::{self, Context, Stopped};
use crate::records::Records;
use crate::tray::Store;
use crate::{open, ttl, upgrade};
use serde_json::{json, Value};
use serve_core::args::{self, Command, Serve, HELP, USAGE};
use serve_core::exit::Exit;
use serve_core::paths::{self, Dirs};
use serve_core::record::Record;
use serve_core::server::{self, Site};
use serve_core::{version, NAME};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

type Out<'a> = &'a mut dyn Write;

pub fn run(argv: &[String], out: Out, err: Out) -> i32 {
    let command = match args::parse(argv) {
        Ok(c) => c,
        Err(args::Usage(message)) => {
            if !message.is_empty() {
                let _ = writeln!(err, "{NAME}: {message}");
            }
            let _ = write!(err, "{USAGE}");
            return Exit::Usage.code();
        }
    };
    let dirs = paths::from_process();
    match command {
        Command::Help => {
            let _ = write!(out, "{USAGE}{HELP}");
            0
        }
        Command::Version => {
            let _ = writeln!(out, "{}", version());
            0
        }
        Command::Upgrade => upgrade::run(out, err),
        Command::Relay => daemon::run(dirs),
        Command::List { json } => with_hub(err, |hub, err| list_cmd(hub, &dirs, json, out, err)),
        Command::Stop { target, json } => with_hub(err, |hub, err| {
            stop_cmd(hub, &dirs, &target, json, out, err)
        }),
        Command::Serve(opts) => serve(&opts, &dirs, out, err),
    }
}

fn with_hub(err: Out, f: impl FnOnce(&Hub, Out) -> i32) -> i32 {
    match Hub::new(&daemon::hub_from_env()) {
        Ok(hub) => f(&hub, err),
        Err(e) => {
            let _ = writeln!(err, "{NAME}: {e}");
            Exit::Failure.code()
        }
    }
}

fn fail(failure: &Failure, opts: Option<&Serve>, out: Out, err: Out) -> i32 {
    for l in &failure.lines {
        let _ = writeln!(err, "{l}");
    }
    if opts.is_some_and(|o| o.json) {
        let _ = writeln!(out, "{}", failure.to_json());
    }
    if let Some(url) = failure.sign_in.as_deref().filter(|u| u.starts_with("http")) {
        let wants = match opts {
            Some(o) if o.open => true,
            _ if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() => {
                let _ = write!(err, "{NAME}: open the sign-in page in your browser? [y/N] ");
                let _ = err.flush();
                let mut answer = String::new();
                let _ = std::io::stdin().lock().read_line(&mut answer);
                matches!(answer.trim(), "y" | "Y" | "yes")
            }
            _ => false,
        };
        if wants {
            if let Err(e) = open::open(url) {
                let _ = writeln!(err, "{NAME}: {e}");
            }
        }
    }
    failure.exit.code()
}

fn list_cmd(hub: &Hub, dirs: &Dirs, json: bool, out: Out, err: Out) -> i32 {
    let store = Store::new(dirs.config.clone());
    let records = Records::new(&dirs.state);
    let ctx = Context {
        hub,
        store: &store,
        records: &records,
        dirs,
    };
    match list::gather(&ctx) {
        Ok(rows) => {
            if json {
                let _ = writeln!(out, "{}", Value::Array(rows));
            } else {
                let _ = write!(out, "{}", serve_core::table::render(&rows));
            }
            0
        }
        Err(f) => fail(&f, None, out, err),
    }
}

fn stop_cmd(hub: &Hub, dirs: &Dirs, target: &str, json: bool, out: Out, err: Out) -> i32 {
    let store = Store::new(dirs.config.clone());
    let records = Records::new(&dirs.state);
    let ctx = Context {
        hub,
        store: &store,
        records: &records,
        dirs,
    };
    match list::stop(&ctx, target) {
        Ok(stopped) => {
            let what = match stopped {
                Stopped::Token(t) | Stopped::Local(t) => t,
            };
            if json {
                let _ = writeln!(out, "{}", json!({ "stopped": what }));
            } else {
                let _ = writeln!(out, "stopped {what}");
            }
            0
        }
        Err(f) => {
            for l in &f.lines {
                let _ = writeln!(err, "{l}");
            }
            if json {
                let _ = writeln!(out, "{}", f.to_json());
            }
            f.exit.code()
        }
    }
}

fn plain(line: String) -> Failure {
    Failure::plain(format!("{NAME}: {line}"))
}

fn directory(dir: &str, entry: Option<&str>) -> Result<PathBuf, Failure> {
    let path = Path::new(dir);
    let abs =
        std::fs::canonicalize(path).map_err(|_| plain(format!("no such directory: {dir}")))?;
    if !abs.is_dir() {
        return Err(plain(format!("not a directory: {dir}")));
    }
    if let Some(entry) = entry {
        let found = serve_core::safe::found(&abs, &abs.join(entry));
        if !found.is_some_and(|(_, meta)| meta.is_file()) {
            return Err(plain(format!(
                "entry file not found: {}",
                Path::new(dir).join(entry).display()
            )));
        }
    }
    Ok(abs)
}

fn shown(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(text)
}

fn serve(opts: &Serve, dirs: &Dirs, out: Out, err: Out) -> i32 {
    let abs = match opts
        .dir
        .as_deref()
        .map(|d| directory(d, opts.entry.as_deref()))
        .transpose()
    {
        Ok(a) => a,
        Err(f) => return fail(&f, Some(opts), out, err),
    };
    if let (Some(dir), Some(ttl_ms)) = (&abs, opts.ttl_ms) {
        return with_hub(err, |hub, err| {
            snapshot(hub, dirs, dir, ttl_ms, opts, out, err)
        });
    }
    if let (true, Some(port)) = (opts.local, opts.port) {
        let url = format!(
            "http://127.0.0.1:{port}/{}",
            opts.entry.as_deref().unwrap_or("")
        );
        warn_unanswered(port, err);
        let record = Record {
            mode: "local".into(),
            local_url: Some(url.clone()),
            port: Some(port),
            ..Record::default()
        };
        return done(&record, opts, out, err, None);
    }
    if let (true, true, Some(dir)) = (opts.local, opts.foreground, &abs) {
        return foreground_local(dir, opts, out, err);
    }
    if let Some(port) = opts.port {
        warn_unanswered(port, err);
    }
    with_hub(err, |hub, err| {
        publish(hub, dirs, abs.as_deref(), opts, out, err)
    })
}

fn warn_unanswered(port: u16, err: Out) {
    let addr = ([127, 0, 0, 1], port).into();
    if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(1)).is_err() {
        let _ = writeln!(err, "{NAME}: nothing answers on 127.0.0.1:{port} yet");
    }
}

fn done(record: &Record, opts: &Serve, out: Out, err: Out, extra: Option<(usize, u64)>) -> i32 {
    let url = record.shown_url().to_string();
    if opts.json {
        let mut value = serde_json::to_value(record).unwrap_or(Value::Null);
        if let Some((files, bytes)) = extra {
            value["files"] = json!(files);
            value["bytes"] = json!(bytes);
        }
        if let Some(map) = value.as_object_mut() {
            map.remove("id");
            map.remove("pid");
            map.remove("servedRoot");
        }
        let _ = writeln!(out, "{value}");
    } else {
        let _ = writeln!(out, "{url}");
        let what = record
            .root
            .clone()
            .or_else(|| record.port.map(|p| format!("127.0.0.1:{p}")))
            .unwrap_or_default();
        let token = record.preview_token.clone();
        match (record.mode.as_str(), token) {
            ("persistent", Some(token)) => {
                let (files, bytes) = extra.unwrap_or_default();
                let _ = writeln!(
                    err,
                    "{NAME}: a snapshot of {what} ({files} files, {}) until {}; stop it sooner with: {NAME} --stop {token}",
                    size(bytes),
                    record.expires_at.as_deref().unwrap_or("-")
                );
            }
            (_, Some(token)) if !opts.foreground => {
                let local = record
                    .local_url
                    .as_deref()
                    .filter(|_| record.root.is_some())
                    .map(|l| format!(" (here: {l})"))
                    .unwrap_or_default();
                let _ = writeln!(err, "{NAME}: {what} is public at {url}{local}");
                let _ = writeln!(err, "{NAME}: stop it with: {NAME} --stop {token}");
            }
            (_, Some(_)) => {
                let _ = writeln!(err, "{NAME}: {what} is public at {url}; Ctrl-C stops it");
            }
            (_, None) if record.root.is_some() && !opts.foreground => {
                let _ = writeln!(
                    err,
                    "{NAME}: serving {what} at {url}; stop it with: {NAME} --stop {url}"
                );
            }
            _ => {}
        }
    }
    if opts.open {
        if let Err(e) = open::open(&url) {
            let _ = writeln!(err, "{NAME}: {e}");
        }
    }
    0
}

fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / 1048576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn snapshot(
    hub: &Hub,
    dirs: &Dirs,
    dir: &Path,
    ttl_ms: u64,
    opts: &Serve,
    out: Out,
    err: Out,
) -> i32 {
    let store = Store::new(dirs.config.clone());
    let records = Records::new(&dirs.state);
    match ttl::publish(hub, &store, &records, dir, opts.entry.as_deref(), ttl_ms) {
        Ok(mut snap) => {
            snap.record.root = Some(shown(dir));
            done(&snap.record, opts, out, err, Some((snap.files, snap.bytes)))
        }
        Err(f) => fail(&f, Some(opts), out, err),
    }
}

fn foreground_local(dir: &Path, opts: &Serve, out: Out, err: Out) -> i32 {
    let site = match Site::new(dir) {
        Ok(s) => Arc::new(s),
        Err(e) => return fail(&plain(format!("{}: {e}", shown(dir))), Some(opts), out, err),
    };
    let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            return fail(
                &plain(format!("can't listen on 127.0.0.1: {e}")),
                Some(opts),
                out,
                err,
            )
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let record = Record {
        mode: "local".into(),
        local_url: Some(format!(
            "http://127.0.0.1:{port}/{}",
            opts.entry.as_deref().unwrap_or("")
        )),
        root: Some(shown(dir)),
        port: Some(port),
        entry: opts.entry.clone(),
        foreground: true,
        ..Record::default()
    };
    done(&record, opts, out, err, None);
    let _ = writeln!(
        err,
        "{NAME}: serving {} at http://127.0.0.1:{port}/; Ctrl-C stops it",
        shown(dir)
    );
    let _ = out.flush();
    let log: server::Log = Arc::new(|req, reply| {
        eprintln!(
            "{NAME}: {} {} {} {}",
            req.method,
            req.target,
            reply.status,
            reply.len()
        );
    });
    server::serve(listener, site, Arc::new(AtomicBool::new(false)), log);
    0
}

fn publish(hub: &Hub, dirs: &Dirs, abs: Option<&Path>, opts: &Serve, out: Out, err: Out) -> i32 {
    let request = json!({
        "op": "publish",
        "dir": abs.map(|p| p.to_string_lossy().into_owned()),
        "port": opts.port,
        "entry": opts.entry,
        "local": opts.local,
        "hold": opts.foreground,
        "hub": hub.origin(),
    });
    let mut exchange = None;
    let mut last = String::new();
    for _ in 0..2 {
        match client::ensure(dirs)
            .and_then(|relay| client::send(&relay, request.clone()).map_err(|e| e.to_string()))
        {
            Ok(x) => {
                exchange = Some(x);
                break;
            }
            Err(e) => last = e,
        }
    }
    let Some(mut exchange) = exchange else {
        return fail(&plain(last), Some(opts), out, err);
    };
    if exchange.reply.get("ok") != Some(&json!(true)) {
        let r = &exchange.reply;
        let failure = Failure {
            exit: Exit::from_code(r.get("exit").and_then(Value::as_i64).unwrap_or(1)),
            lines: r
                .get("lines")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            sign_in: r.get("signIn").and_then(Value::as_str).map(str::to_string),
        };
        return fail(&failure, Some(opts), out, err);
    }
    let mut record: Record =
        serde_json::from_value(exchange.reply["record"].clone()).unwrap_or_default();
    if let Some(dir) = abs {
        record.root = Some(shown(dir));
    }
    done(&record, opts, out, err, None);
    if !opts.foreground {
        return 0;
    }
    let _ = out.flush();
    let mut text = String::new();
    loop {
        text.clear();
        match exchange.reader.read_line(&mut text) {
            Ok(0) | Err(_) => {
                let _ = writeln!(err, "{NAME}: the relay went away");
                return Exit::Failure.code();
            }
            Ok(_) => {}
        }
        let Ok(message) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(log) = message.get("log").and_then(Value::as_str) {
            let _ = writeln!(err, "{NAME}: {log}");
        }
        if let Some(reason) = message.get("stopped").and_then(Value::as_str) {
            let _ = writeln!(err, "{NAME}: stopped: {reason}");
            return 0;
        }
    }
}

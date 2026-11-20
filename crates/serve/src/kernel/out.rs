use super::embedder::{self, Error};
use serde_json::{json, Value};
use serve_core::args::Serve;
use serve_core::exit::Exit;
use serve_core::record::Record;

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

    pub fn page(cmd: &str, error: &Error) -> Failure {
        Failure {
            exit: Exit::Failure,
            lines: error.lines(cmd),
            sign_in: None,
        }
    }

    pub fn hub(cmd: &str, answer: &Value, doing: &str) -> Failure {
        let text = |k: &str| answer.get(k).and_then(Value::as_str).map(str::to_string);
        let status = answer.get("status").and_then(Value::as_u64).unwrap_or(0);
        let mut error = text("error").unwrap_or_else(|| "the tray hub failed".into());
        match text("code").as_deref() {
            Some("LIVE_PREVIEW_LIMIT") => {
                let count = |k: &str| answer.get(k).and_then(Value::as_u64);
                if let (Some(active), Some(limit)) = (count("active"), count("limit")) {
                    error.push_str(&format!(" ({active} of {limit} live previews in use)"));
                }
                error.push_str(&format!(
                    "; list them with `{cmd} --list` and stop the ones you no longer need with `{cmd} --stop <token>`"
                ))
            }
            Some(code) => error.push_str(&format!(" ({code})")),
            None => {}
        }
        let sign_in = text("signIn");
        let (exit, mut lines) = match status {
            401 | 403 => (
                Exit::Auth,
                vec![format!(
                    "{cmd}: {doing}: the tray hub refused it ({status}: {error})"
                )],
            ),
            402 | 429 => (
                Exit::Quota,
                vec![format!(
                    "{cmd}: {doing}: the tray hub's limit is reached ({status}: {error})"
                )],
            ),
            _ if sign_in.is_some() => (
                Exit::Auth,
                vec![format!(
                    "{cmd}: the tray hub wants you to sign in: {}",
                    sign_in.clone().unwrap_or_default()
                )],
            ),
            _ => (
                Exit::Failure,
                vec![format!(
                    "{cmd}: {doing}: the tray hub answered {status}: {error}"
                )],
            ),
        };
        if exit == Exit::Auth && matches!(status, 401 | 403) {
            if let Some(url) = &sign_in {
                lines.push(format!("{cmd}: sign in at {url}"));
            }
        }
        if exit == Exit::Quota {
            if let Some(after) = text("retryAfter").or_else(|| {
                answer
                    .get("retryAfter")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
            }) {
                let after = if after.bytes().all(|b| b.is_ascii_digit()) {
                    format!("{after} s")
                } else {
                    after
                };
                lines.push(format!("{cmd}: retry after {after}"));
            }
        }
        Failure {
            exit,
            lines,
            sign_in,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "ok": false, "exit": self.exit.code(), "lines": self.lines, "signIn": self.sign_in })
    }

    pub fn from_json(value: &Value) -> Failure {
        Failure {
            exit: Exit::from_code(value.get("exit").and_then(Value::as_i64).unwrap_or(1)),
            lines: value
                .get("lines")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            sign_in: value
                .get("signIn")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }
}

pub fn fail(failure: &Failure, json: bool) -> i32 {
    for line in &failure.lines {
        eprintln!("{line}");
    }
    if json {
        println!("{}", failure.to_json());
    }
    failure.exit.code()
}

pub fn page_url(port: u16) -> String {
    match std::env::var("SLICC_PAGE_LOOPBACK_URL") {
        Ok(t) if t.contains("{port}") => format!(
            "{}/",
            t.replace("{port}", &port.to_string()).trim_end_matches('/')
        ),
        _ => format!("http://127.0.0.1:{port}/"),
    }
}

pub fn done(cmd: &str, record: &Record, opts: &Serve) -> i32 {
    let url = record.shown_url().to_string();
    if opts.json {
        let mut value = serde_json::to_value(record).unwrap_or(Value::Null);
        if let Some(map) = value.as_object_mut() {
            map.remove("id");
            map.remove("pid");
            map.remove("servedRoot");
        }
        println!("{value}");
    } else {
        println!("{url}");
        let what = record
            .root
            .clone()
            .or_else(|| record.port.map(|p| format!("127.0.0.1:{p}")))
            .unwrap_or_default();
        match (&record.preview_token, record.foreground) {
            (Some(token), false) => {
                let here = record
                    .local_url
                    .as_deref()
                    .filter(|_| record.root.is_some())
                    .map(|l| format!(" (here: {l})"))
                    .unwrap_or_default();
                eprintln!("{cmd}: {what} is public at {url}{here}");
                eprintln!("{cmd}: stop it with: {cmd} --stop {token}");
            }
            (Some(_), true) => eprintln!("{cmd}: {what} is public at {url}; Ctrl-C stops it"),
            (None, false) if record.root.is_some() => {
                eprintln!("{cmd}: serving {what} at {url}; stop it with: {cmd} --stop {url}")
            }
            (None, true) if record.root.is_some() => {
                eprintln!("{cmd}: serving {what} at {url}; Ctrl-C stops it")
            }
            _ => {}
        }
    }
    if opts.open {
        if let Err(error) = embedder::tell(&json!({ "kind": "open.url", "url": url })) {
            for line in error.lines(cmd) {
                eprintln!("{line}");
            }
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    0
}

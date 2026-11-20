use crate::MAX_TTL_MS;

pub const USAGE: &str = "usage: scserve [--entry FILE] [--ttl DURATION] [--local] [--foreground] [--open] [--json] DIRECTORY
       scserve --port PORT [--entry PATH] [--local] [--foreground] [--open] [--json]
       scserve --list [--json]
       scserve --stop TOKEN|URL [--json]
       scserve --version | --upgrade | --help
";

pub const HELP: &str = "
Serve DIRECTORY, or publish the server on 127.0.0.1:PORT, at a public
capability URL on sliccy.now through the SLICC tray hub.

  DIRECTORY        serve its files with the built-in static server
  --port PORT      publish a server that already runs on 127.0.0.1:PORT
  --entry FILE     the page the URL opens (default: the directory's index)
  --ttl DURATION   publish an immutable snapshot for DURATION (m, h, d or w,
                   at most 30d); nothing keeps running
  --local          stay on this machine: print http://127.0.0.1:PORT/ only
  --foreground     stay attached; Ctrl-C stops the preview
  --open           open the URL with the platform's opener
  --json           print the result as JSON
  --list           list the previews this machine published
  --stop TOKEN     stop a preview, by token or URL
  --version        print the version
  --upgrade        upgrade scserve in place
  -h, --help       print this help

Exit status: 0 ok, 1 failure, 2 usage error, 3 the tray hub wants a sign-in,
4 a quota of the tray hub is used up.
";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Serve {
    pub dir: Option<String>,
    pub port: Option<u16>,
    pub entry: Option<String>,
    pub ttl_ms: Option<u64>,
    pub local: bool,
    pub foreground: bool,
    pub open: bool,
    pub json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Upgrade,
    Relay,
    List { json: bool },
    Stop { target: String, json: bool },
    Serve(Serve),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage(pub String);

const LATER: &[&str] = &[
    "--bridge",
    "--no-bridge",
    "--max-tabs",
    "--quiet",
    "--logs",
    "--truncate",
    "--lines",
];

#[derive(Default)]
struct State {
    serve: Serve,
    list: bool,
    stop: Option<String>,
    version: bool,
    upgrade: bool,
    count: usize,
}

pub fn parse(args: &[String]) -> Result<Command, Usage> {
    if args.first().map(String::as_str) == Some("__relay") && args.len() == 1 {
        return Ok(Command::Relay);
    }
    if args.is_empty() {
        return Err(Usage(String::new()));
    }
    if args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "-h" || a == "--help")
    {
        return Ok(Command::Help);
    }
    let mut state = State::default();
    let mut i = 0;
    let mut options = true;
    while i < args.len() {
        let arg = args[i].as_str();
        i += 1;
        if options && arg == "--" {
            options = false;
            continue;
        }
        if !options || !arg.starts_with('-') || arg == "-" {
            if state.serve.dir.is_some() {
                return Err(Usage("give one directory".into()));
            }
            state.serve.dir = Some(arg.to_string());
            state.count += 1;
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (arg, None),
        };
        let mut value = |what: &str| -> Result<String, Usage> {
            if let Some(v) = inline.clone() {
                return if v.is_empty() {
                    Err(Usage(format!("{name} needs {what}")))
                } else {
                    Ok(v)
                };
            }
            match args.get(i) {
                Some(v) if !v.is_empty() => {
                    i += 1;
                    Ok(v.clone())
                }
                _ => Err(Usage(format!("{name} needs {what}"))),
            }
        };
        state.count += 1;
        match name {
            "--entry" => state.serve.entry = Some(entry(&value("a file")?)?),
            "--ttl" => state.serve.ttl_ms = Some(ttl(&value("a duration")?)?),
            "--port" => state.serve.port = Some(port(&value("a port")?)?),
            "--stop" => state.stop = Some(value("a token or URL")?),
            _ if inline.is_some() => return Err(Usage(format!("{name} takes no value"))),
            "--local" => state.serve.local = true,
            "--foreground" => state.serve.foreground = true,
            "--open" => state.serve.open = true,
            "--json" => state.serve.json = true,
            "--list" => state.list = true,
            "--version" => state.version = true,
            "--upgrade" => state.upgrade = true,
            "--project" => {
                return Err(Usage(
                    "--project is obsolete: give the directory to serve itself".into(),
                ))
            }
            _ if LATER.contains(&name) => {
                return Err(Usage(format!("{name} is not supported yet")));
            }
            _ => return Err(Usage(format!("unknown option: {name}"))),
        }
    }
    finish(state)
}

fn finish(state: State) -> Result<Command, Usage> {
    let serve = state.serve;
    for (on, flag) in [(state.version, "--version"), (state.upgrade, "--upgrade")] {
        if on {
            return if state.count == 1 {
                Ok(if flag == "--version" {
                    Command::Version
                } else {
                    Command::Upgrade
                })
            } else {
                Err(Usage(format!("{flag} takes no other arguments")))
            };
        }
    }
    let extra = state.count - usize::from(serve.json);
    if state.list {
        return if extra == 1 {
            Ok(Command::List { json: serve.json })
        } else {
            Err(Usage("--list takes only --json".into()))
        };
    }
    if let Some(target) = state.stop {
        return if extra == 1 {
            Ok(Command::Stop {
                target,
                json: serve.json,
            })
        } else {
            Err(Usage("--stop takes only --json".into()))
        };
    }
    match (&serve.dir, serve.port) {
        (None, None) => return Err(Usage("give a directory or --port".into())),
        (Some(_), Some(_)) => return Err(Usage("give a directory or --port, not both".into())),
        _ => {}
    }
    if serve.ttl_ms.is_some() {
        if serve.port.is_some() {
            return Err(Usage(
                "--ttl snapshots a directory; it can't snapshot a server".into(),
            ));
        }
        if serve.local {
            return Err(Usage("--ttl publishes; it can't be --local".into()));
        }
        if serve.foreground {
            return Err(Usage(
                "--ttl leaves nothing running, so there is no --foreground".into(),
            ));
        }
    }
    Ok(Command::Serve(serve))
}

pub fn entry(raw: &str) -> Result<String, Usage> {
    let bad = || Usage(format!("invalid entry: {raw}"));
    if raw.starts_with('/') || raw.starts_with('\\') || raw.contains('\0') || raw.contains(':') {
        return Err(bad());
    }
    let parts: Vec<&str> = raw
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.is_empty() || parts.contains(&"..") {
        return Err(bad());
    }
    Ok(parts.join("/"))
}

pub fn ttl(raw: &str) -> Result<u64, Usage> {
    let bad = || Usage("--ttl must be a positive whole duration (m, h, d or w)".into());
    let unit = match raw.chars().last() {
        Some('m') => 60_000u64,
        Some('h') => 3_600_000,
        Some('d') => 86_400_000,
        Some('w') => 604_800_000,
        _ => return Err(bad()),
    };
    let digits = &raw[..raw.len() - 1];
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    let n: u64 = digits.parse().map_err(|_| bad())?;
    let ms = n.checked_mul(unit).ok_or_else(bad)?;
    if ms > MAX_TTL_MS {
        return Err(Usage("--ttl can't exceed 30d".into()));
    }
    Ok(ms)
}

pub fn port(raw: &str) -> Result<u16, Usage> {
    match raw.parse::<u16>() {
        Ok(p) if p > 0 && raw.bytes().all(|b| b.is_ascii_digit()) => Ok(p),
        _ => Err(Usage(format!(
            "--port must be a port from 1 to 65535: {raw}"
        ))),
    }
}

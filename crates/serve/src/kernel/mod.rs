mod embedder;
mod hold;
mod out;
mod page;
mod store;
mod sys;
mod ttl;

use hold::Spec;
use out::Failure;
use serve_core::args::{self, Command, Serve, HELP, USAGE};
use serve_core::record::Record;
use serve_core::{version, PACKAGE};
use std::path::Path;

const NAME: &str = "serve";

fn name() -> String {
    std::env::args()
        .next()
        .and_then(|a| a.rsplit('/').next().map(str::to_string))
        .filter(|n| n == "scserve")
        .unwrap_or_else(|| NAME.into())
}

pub fn run(argv: &[String]) -> i32 {
    let cmd = name();
    if let Ok(pwd) = std::env::var("PWD") {
        let _ = std::env::set_current_dir(pwd);
    }
    if argv.len() == 2 && argv[0] == hold::ARG {
        return hold::background(&cmd, &argv[1]);
    }
    let usage = USAGE.replace("scserve", &cmd);
    match args::parse(argv) {
        Err(args::Usage(message)) => {
            if !message.is_empty() {
                eprintln!("{cmd}: {message}");
            }
            eprint!("{usage}");
            2
        }
        Ok(Command::Help) => {
            print!("{usage}{}", HELP.replace("scserve", &cmd));
            0
        }
        Ok(Command::Version) => {
            println!("{}", version());
            0
        }
        Ok(Command::Upgrade) => {
            println!("pnpm add -g {PACKAGE}@latest");
            0
        }
        Ok(Command::Relay) => {
            eprint!("{usage}");
            2
        }
        Ok(Command::List { json }) => page::list(&cmd, json),
        Ok(Command::Stop { target, json }) => page::stop(&cmd, &target, json),
        Ok(Command::Serve(opts)) => serve(&cmd, &opts),
    }
}

fn directory(cmd: &str, dir: &str, entry: Option<&str>) -> Result<String, Failure> {
    let abs = std::fs::canonicalize(dir)
        .map_err(|_| Failure::plain(format!("{cmd}: no such directory: {dir}")))?;
    if !abs.is_dir() {
        return Err(Failure::plain(format!("{cmd}: not a directory: {dir}")));
    }
    if let Some(entry) = entry {
        let found = serve_core::safe::found(&abs, &abs.join(entry));
        if !found.is_some_and(|(_, meta)| meta.is_file()) {
            return Err(Failure::plain(format!(
                "{cmd}: entry file not found: {}",
                Path::new(dir).join(entry).display()
            )));
        }
    }
    Ok(abs.to_string_lossy().into_owned())
}

fn warn_unanswered(cmd: &str, port: u16) {
    if wasix_net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        eprintln!("{cmd}: nothing answers on 127.0.0.1:{port} yet");
    }
}

fn serve(cmd: &str, opts: &Serve) -> i32 {
    if let (true, Some(port)) = (opts.local, opts.port) {
        warn_unanswered(cmd, port);
        let record = Record {
            mode: "local".into(),
            local_url: Some(format!(
                "{}{}",
                out::page_url(port),
                opts.entry.as_deref().unwrap_or("")
            )),
            port: Some(port),
            created_at: serve_core::time::iso(serve_core::time::now_ms()),
            ..Record::default()
        };
        return out::done(cmd, &record, opts);
    }
    let dir = match opts
        .dir
        .as_deref()
        .map(|d| directory(cmd, d, opts.entry.as_deref()))
        .transpose()
    {
        Ok(d) => d,
        Err(f) => return out::fail(&f, opts.json),
    };
    if let (Some(dir), Some(ttl_ms)) = (&dir, opts.ttl_ms) {
        return ttl::publish(cmd, dir, ttl_ms, opts);
    }
    if let (None, Some(port)) = (&dir, opts.port) {
        warn_unanswered(cmd, port);
    }
    let spec = Spec {
        dir,
        port: opts.port,
        entry: opts.entry.clone(),
        local: opts.local,
        foreground: opts.foreground,
    };
    if opts.foreground {
        hold::foreground(cmd, &spec, opts)
    } else {
        hold::detach(cmd, &spec, opts)
    }
}

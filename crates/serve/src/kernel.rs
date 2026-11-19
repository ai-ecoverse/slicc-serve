use serve_core::args::{self, Command, Serve, HELP, USAGE};
use serve_core::server::{self, Site};
use serve_core::{version, PACKAGE};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
        Ok(Command::Serve(opts)) if opts.local && opts.ttl_ms.is_none() => local(&cmd, &opts),
        Ok(_) => {
            eprintln!(
                "{cmd}: publishing from SLICC needs seven's serve.publish, which isn't there yet; only --local works in SLICC so far"
            );
            1
        }
    }
}

fn page_url(port: u16) -> String {
    match std::env::var("SLICC_PAGE_LOOPBACK_URL") {
        Ok(t) if t.contains("{port}") => format!(
            "{}/",
            t.replace("{port}", &port.to_string()).trim_end_matches('/')
        ),
        _ => format!("http://127.0.0.1:{port}/"),
    }
}

fn local(cmd: &str, opts: &Serve) -> i32 {
    let entry = opts.entry.as_deref().unwrap_or("");
    if let Some(port) = opts.port {
        let url = format!("{}{entry}", page_url(port));
        println!("{url}");
        return 0;
    }
    let dir = opts.dir.as_deref().unwrap_or(".");
    let site = match Site::new(std::path::Path::new(dir)) {
        Ok(s) => Arc::new(s),
        Err(_) => {
            eprintln!("{cmd}: no such directory: {dir}");
            return 1;
        }
    };
    let listener = match wasix_net::TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{cmd}: can't listen on 127.0.0.1: {e}");
            return 1;
        }
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    println!("{}{entry}", page_url(port));
    eprintln!("{cmd}: serving {dir} in the foreground; Ctrl-C stops it");
    let log: server::Log = Arc::new(|req, reply| {
        eprintln!("{} {} {}", req.method, req.target, reply.status);
    });
    server::serve(listener, site, Arc::new(AtomicBool::new(false)), log);
    0
}

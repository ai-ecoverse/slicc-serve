use crate::{detach, fsx};
use serde_json::{json, Value};
use serve_core::paths::Dirs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Relay {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

pub struct Exchange {
    pub reply: Value,
    pub reader: BufReader<TcpStream>,
}

fn info(dirs: &Dirs) -> Option<Relay> {
    let v: Value = fsx::read_json(&dirs.state.join("relay.json"))?;
    Some(Relay {
        port: u16::try_from(v.get("port")?.as_u64()?).ok()?,
        token: v.get("token")?.as_str()?.to_string(),
        pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
    })
}

pub fn send(relay: &Relay, mut request: Value) -> std::io::Result<Exchange> {
    request["token"] = json!(relay.token);
    let mut stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], relay.port).into(), Duration::from_secs(2))?;
    let mut text = request.to_string();
    text.push('\n');
    stream.write_all(text.as_bytes())?;
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let reply = serde_json::from_str(&first)
        .map_err(|_| std::io::Error::other("the relay sent no answer"))?;
    Ok(Exchange { reply, reader })
}

pub fn find(dirs: &Dirs) -> Option<Relay> {
    let relay = info(dirs)?;
    let exchange = send(&relay, json!({ "op": "ping" })).ok()?;
    (exchange.reply.get("ok") == Some(&json!(true))).then_some(relay)
}

pub fn ensure(dirs: &Dirs) -> Result<Relay, String> {
    if let Some(relay) = find(dirs) {
        return Ok(relay);
    }
    detach::spawn_relay(&dirs.state.join("relay.log"))
        .map_err(|e| format!("can't start the relay: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        if let Some(relay) = find(dirs) {
            return Ok(relay);
        }
    }
    let log = std::fs::read_to_string(dirs.state.join("relay.log")).unwrap_or_default();
    Err(format!(
        "the relay didn't start{}",
        log.lines()
            .last()
            .map(|l| format!(": {l}"))
            .unwrap_or_default()
    ))
}

pub fn alive(dirs: &Dirs, pid: Option<u32>) -> bool {
    match (find(dirs), pid) {
        (Some(relay), Some(pid)) => relay.pid == pid,
        _ => false,
    }
}

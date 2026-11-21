use serde_json::{json, Value};

pub fn render(rows: &[Value]) -> String {
    if rows.is_empty() {
        return "no previews\n".into();
    }
    let cell = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("-").to_string();
    let lines: Vec<[String; 5]> = std::iter::once([
        "TOKEN".to_string(),
        "MODE".into(),
        "EXPIRES".into(),
        "URL".into(),
        "ROOT".into(),
    ])
    .chain(rows.iter().map(|r| {
        let token = r
            .get("previewToken")
            .and_then(Value::as_str)
            .or_else(|| r.get("id").and_then(Value::as_str))
            .unwrap_or("-")
            .to_string();
        let mode = if r.get("foreground") == Some(&json!(true)) {
            format!("{} (foreground)", cell(r, "mode"))
        } else {
            cell(r, "mode")
        };
        let root = match (
            r.get("root").and_then(Value::as_str),
            r.get("localUrl").and_then(Value::as_str),
        ) {
            (Some(root), _) => root.to_string(),
            (None, Some(local)) if cell(r, "mode") != "local" => local.to_string(),
            _ => "-".into(),
        };
        [token, mode, cell(r, "expiresAt"), cell(r, "url"), root]
    }))
    .collect();
    let widths: Vec<usize> = (0..4)
        .map(|i| {
            lines
                .iter()
                .map(|l| l[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for l in &lines {
        for (i, w) in widths.iter().enumerate() {
            out.push_str(&format!("{:<w$}  ", l[i]));
        }
        out.push_str(&l[4]);
        out.push('\n');
    }
    out
}

pub fn printable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c < ' ' || c == '\u{7f}' || ('\u{80}'..='\u{9f}').contains(&c) {
            out.push_str(&format!("\\u{:04x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

pub fn log_line(r: &Value) -> String {
    let t = |k: &str| printable(r.get(k).and_then(Value::as_str).unwrap_or("-"));
    let event = t("event");
    let disposition = match (event.as_str(), r.get("announced") == Some(&json!(true))) {
        ("connected", true) => "announced",
        ("connected", false) => "suppressed",
        _ => "-",
    };
    let detail = match event.as_str() {
        "connected" => format!("origin={} userAgent={}", t("origin"), t("userAgent")),
        "disconnected" => format!("reason={}", t("reason")),
        _ => format!("name={} detail={}", t("name"), t("detail")),
    };
    format!(
        "{}  {:<12} {:<10} preview={} conn={} {detail}",
        t("at"),
        event,
        disposition,
        t("previewToken"),
        t("connId")
    )
}

pub fn event_line(cmd: &str, ev: &Value, url: &str) -> Vec<String> {
    let t = |k: &str| printable(ev.get(k).and_then(Value::as_str).unwrap_or("-"));
    let mut lines = Vec::new();
    match t("event").as_str() {
        "connected" => {
            if ev.get("announced") == Some(&json!(true)) {
                lines.push(format!(
                    "{cmd}: first visitor on {url}: {} ({})",
                    t("origin"),
                    t("userAgent")
                ));
            }
            lines.push(format!(
                "{cmd}: connected {} {} {}",
                t("connId"),
                t("origin"),
                t("userAgent")
            ));
        }
        "disconnected" => lines.push(format!(
            "{cmd}: disconnected {} {}",
            t("connId"),
            t("reason")
        )),
        _ => lines.push(format!(
            "{cmd}: emit {} {} {}",
            t("connId"),
            t("name"),
            t("detail")
        )),
    }
    lines
}

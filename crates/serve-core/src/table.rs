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

use serde_json::{json, Value};

pub const HANDLE: &str = "scserve";
pub const GONE: &str =
    "the object is gone from the visitor's page (it navigated, or the handle was released)";

const REGISTRY: &str = "const R = (globalThis[Symbol.for('scserve.bridge')] ??= { h: new Map(), n: 0, nodes: new Map(), ids: new WeakMap(), b: 0 }); \
const keep = (v) => { const id = String(++R.n); R.h.set(id, v); return id; }; \
const get = (id) => { if (!R.h.has(id)) throw new Error('scserve: gone'); return R.h.get(id); }; \
const node = (b) => { if (!R.nodes.has(b)) throw new Error('scserve: gone'); return R.nodes.get(b); }; \
const bid = (n) => { let b = R.ids.get(n); if (b === undefined) { b = ++R.b; R.ids.set(n, b); R.nodes.set(b, n); } return b; }; \
const desc = (v) => { const t = typeof v; const o = v !== null && (t === 'object' || t === 'function'); \
return { t, s: v === null ? 'null' : Array.isArray(v) ? 'array' : v instanceof Error ? 'error' : typeof Node !== 'undefined' && v instanceof Node ? 'node' : undefined, \
c: o && v.constructor ? v.constructor.name : undefined, d: o ? String(v).slice(0, 1000) : undefined, \
v: v === null || t === 'string' || t === 'boolean' || (t === 'number' && Number.isFinite(v)) ? v : undefined, \
u: t === 'bigint' ? v + 'n' : t === 'number' && !Number.isFinite(v) ? (Object.is(v, -0) ? '-0' : String(v)) : t === 'number' && Object.is(v, -0) ? '-0' : undefined, \
h: o ? keep(v) : undefined }; }; \
const plain = (v) => { const t = typeof v; \
if (t === 'number' && (!Number.isFinite(v) || Object.is(v, -0))) return { t, u: Object.is(v, -0) ? '-0' : String(v) }; \
if (t === 'bigint') return { t, u: v + 'n' }; if (t === 'undefined' || t === 'function' || t === 'symbol') return { t }; \
return { t, v, s: v === null ? 'null' : Array.isArray(v) ? 'array' : undefined }; };";

fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

fn wrap(body: &str) -> String {
    format!("(() => {{ {REGISTRY} {body} }})()")
}

pub fn handle_id(conn: &str, page: &str) -> String {
    format!("{HANDLE}:{conn}:{page}")
}

pub fn page_id<'a>(conn: &str, object_id: &'a str) -> Option<&'a str> {
    object_id
        .strip_prefix(HANDLE)?
        .strip_prefix(':')?
        .strip_prefix(conn)?
        .strip_prefix(':')
}

pub fn is_handle(object_id: &str) -> bool {
    object_id.starts_with(&format!("{HANDLE}:"))
}

pub fn evaluate(source: &str, by_value: bool) -> String {
    let finish = if by_value { "plain" } else { "desc" };
    wrap(&format!(
        "return Promise.resolve((0, eval)({})).then((v) => JSON.stringify({finish}(v)));",
        literal(source)
    ))
}

pub fn argument(conn: &str, arg: &Value) -> Option<String> {
    if let Some(id) = arg.get("objectId").and_then(Value::as_str) {
        return page_id(conn, id).map(|p| format!("get({})", literal(p)));
    }
    if let Some(u) = arg.get("unserializableValue").and_then(Value::as_str) {
        return Some(match u {
            "NaN" | "Infinity" | "-Infinity" | "-0" => u.to_string(),
            _ if u.ends_with('n')
                && u[..u.len() - 1]
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b == b'-') =>
            {
                format!("BigInt({})", literal(&u[..u.len() - 1]))
            }
            _ => return None,
        });
    }
    Some(match arg.get("value") {
        Some(v) => v.to_string(),
        None => "undefined".into(),
    })
}

pub fn call(this: Option<&str>, function: &str, args: &[String], by_value: bool) -> String {
    let finish = if by_value { "plain" } else { "desc" };
    let this = this
        .map(|p| format!("get({})", literal(p)))
        .unwrap_or_else(|| "undefined".into());
    let source = literal(&format!("({function})"));
    wrap(&format!(
        "const f = (0, eval)({source}); return Promise.resolve(Reflect.apply(f, {this}, [{}])).then((v) => JSON.stringify({finish}(v)));",
        args.join(", ")
    ))
}

pub fn properties(page: &str, accessors_only: bool) -> String {
    wrap(&format!(
        "const o = get({}); const all = Object.getOwnPropertyDescriptors(o); \
return JSON.stringify(Reflect.ownKeys(all).filter((k) => typeof k === 'string').flatMap((name) => {{ const d = all[name]; \
const accessor = 'get' in d || 'set' in d; if ({accessors_only} && !accessor) return []; \
const p = {{ name, configurable: d.configurable, enumerable: d.enumerable }}; \
if (accessor) {{ if (d.get !== undefined) p.get = desc(d.get); if (d.set !== undefined) p.set = desc(d.set); }} \
else {{ p.value = desc(d.value); p.writable = d.writable; }} return [p]; }}));",
        literal(page)
    ))
}

pub fn release(pages: &[String]) -> String {
    let ids: Vec<String> = pages.iter().map(|p| literal(p)).collect();
    wrap(&format!(
        "for (const id of [{}]) R.h.delete(id); return '[]';",
        ids.join(", ")
    ))
}

pub fn describe(page: Option<&str>, backend: Option<u64>) -> String {
    let target = match (page, backend) {
        (Some(p), _) => format!("get({})", literal(p)),
        (None, Some(b)) => format!("node({b})"),
        _ => "undefined".into(),
    };
    wrap(&format!(
        "const n = {target}; if (typeof Node === 'undefined' || !(n instanceof Node)) throw new Error('scserve: not a node'); \
return JSON.stringify({{ backendNodeId: bid(n), nodeType: n.nodeType, nodeName: n.nodeName, localName: n.localName || '', nodeValue: n.nodeValue || '', \
childNodeCount: n.childNodes ? n.childNodes.length : 0, attributes: n.attributes ? Array.from(n.attributes).flatMap((a) => [a.name, a.value]) : [] }});"
    ))
}

pub fn resolve(backend: u64) -> String {
    wrap(&format!("return JSON.stringify(desc(node({backend})));"))
}

fn unpack(result: &Value) -> Option<Value> {
    result
        .pointer("/result/value")
        .and_then(Value::as_str)
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
}

fn failure(result: &Value) -> Option<Result<Value, (i64, String)>> {
    let details = result.get("exceptionDetails")?;
    let text = details.get("text").and_then(Value::as_str).unwrap_or("");
    if text.starts_with("scserve: gone") || text.contains("scserve: gone") {
        return Some(Err((-32000, GONE.into())));
    }
    if text.contains("scserve: not a node") {
        return Some(Err((-32000, "the object is not a DOM node".into())));
    }
    Some(Ok(result.clone()))
}

pub fn remote(conn: &str, d: &Value) -> Value {
    let kind = d.get("t").and_then(Value::as_str).unwrap_or("undefined");
    let mut out = json!({ "type": kind });
    for (from, to) in [
        ("s", "subtype"),
        ("c", "className"),
        ("d", "description"),
        ("u", "unserializableValue"),
    ] {
        if let Some(v) = d.get(from).filter(|v| !v.is_null()) {
            out[to] = v.clone();
        }
    }
    if out.get("unserializableValue").is_none() && kind != "undefined" {
        if let Some(v) = d.get("v") {
            out["value"] = v.clone();
        } else if d.get("s").and_then(Value::as_str) == Some("null") {
            out["value"] = Value::Null;
        }
    }
    if let Some(h) = d.get("h").and_then(Value::as_str) {
        out["objectId"] = json!(handle_id(conn, h));
    }
    out
}

pub fn result(conn: &str, reply: Value) -> Result<Value, (i64, String)> {
    if let Some(f) = failure(&reply) {
        return f;
    }
    match unpack(&reply) {
        Some(d) => Ok(json!({ "result": remote(conn, &d) })),
        None => Ok(reply),
    }
}

pub fn listed(conn: &str, reply: Value) -> Result<Value, (i64, String)> {
    if let Some(f) = failure(&reply) {
        return f.and(Err((
            -32000,
            "the object's properties can't be read".into(),
        )));
    }
    let items = unpack(&reply)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let props: Vec<Value> = items
        .iter()
        .map(|p| {
            let mut out = json!({
                "name": p.get("name").cloned().unwrap_or(Value::Null),
                "configurable": p.get("configurable").cloned().unwrap_or(json!(true)),
                "enumerable": p.get("enumerable").cloned().unwrap_or(json!(true)),
                "isOwn": true,
            });
            for k in ["value", "get", "set"] {
                if let Some(v) = p.get(k) {
                    out[k] = remote(conn, v);
                }
            }
            if let Some(w) = p.get("writable") {
                out["writable"] = w.clone();
            }
            out
        })
        .collect();
    Ok(json!({ "result": props }))
}

pub fn described(reply: Value) -> Result<Value, (i64, String)> {
    if let Some(f) = failure(&reply) {
        return f.and(Err((-32000, "the node can't be described".into())));
    }
    let mut node = unpack(&reply).unwrap_or(Value::Null);
    if !node.is_object() {
        return Err((-32000, "the node can't be described".into()));
    }
    node["nodeId"] = json!(0);
    Ok(json!({ "node": node }))
}

pub fn resolved(conn: &str, reply: Value) -> Result<Value, (i64, String)> {
    if let Some(f) = failure(&reply) {
        return f.and(Err((-32000, GONE.into())));
    }
    match unpack(&reply) {
        Some(d) => Ok(json!({ "object": remote(conn, &d) })),
        None => Err((-32000, GONE.into())),
    }
}

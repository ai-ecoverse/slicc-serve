const TYPES: &[(&str, &str)] = &[
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("cjs", "text/javascript; charset=utf-8"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("webmanifest", "application/manifest+json"),
    ("txt", "text/plain; charset=utf-8"),
    ("md", "text/markdown; charset=utf-8"),
    ("csv", "text/csv; charset=utf-8"),
    ("xml", "application/xml"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("bmp", "image/bmp"),
    ("woff2", "font/woff2"),
    ("woff", "font/woff"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("flac", "audio/flac"),
    ("pdf", "application/pdf"),
    ("wasm", "application/wasm"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
];

pub fn for_path(path: &str) -> &'static str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((_, ext)) = name.rsplit_once('.') else {
        return "application/octet-stream";
    };
    let ext = ext.to_ascii_lowercase();
    TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, t)| *t)
        .unwrap_or("application/octet-stream")
}

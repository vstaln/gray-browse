//! gray-browse — keyless web fetching for the agent.
//!
//! Port of the keyless half of hermes's browser plugins (firecrawl +
//! browser_use): a `browse` tool that fetches a URL with curl (follows
//! redirects, 20s timeout, 8 MiB cap) and extracts readable text in Rust —
//! script/style/tags stripped, whitespace collapsed, links kept inline as
//! `[text](href)`. `action:"links"` returns just the link list,
//! `action:"raw"` the first 64 KiB of untouched HTML.
//!
//! `provider:"firecrawl"` (or auto + FIRECRAWL_API_KEY set) goes through
//! Firecrawl's /scrape endpoint instead — the JS-heavy path. Tool errors are
//! friendly: the HTTP status is reported, never a panic.

use std::io::{BufRead, Read, Write};

use serde_json::{Value, json};

const MAX_BODY: usize = 8 * 1024 * 1024;
const RAW_CAP: usize = 64 * 1024;
const UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) gray-browse/0.1 (+https://github.com/vstaln/gray)";

fn manifest() -> Value {
    json!({
        "name": "browse",
        "version": env!("CARGO_PKG_VERSION"),
        "protocol": "1.1",
        "tools": [{
            "name": "browse",
            "description": "Fetch a web page and return readable text — links kept inline as [text](href). action 'links' returns only the link list, 'raw' the first 64KB of HTML. provider 'firecrawl' uses the Firecrawl scrape API (needs FIRECRAWL_API_KEY) for JS-heavy pages.",
            "parameters": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http(s) URL to fetch." },
                    "action": { "type": "string", "enum": ["read", "links", "raw"],
                                "description": "read (default): readable text. links: link list only. raw: first 64KB of HTML." },
                    "provider": { "type": "string", "enum": ["auto", "curl", "firecrawl"],
                                  "description": "auto (default): firecrawl when FIRECRAWL_API_KEY is set, else curl." }
                },
                "required": ["url"]
            }
        }],
        "commands": [],
    })
}

struct Fetch {
    status: Option<u16>,
    body: Vec<u8>,
}

/// curl → body capped at MAX_BODY + last HTTP status from dumped headers.
fn fetch(url: &str) -> Result<Fetch, String> {
    let mut child = std::process::Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--max-time",
            "20",
            "--compressed",
            "-D",
            "/dev/stderr",
            "-A",
            UA,
            "--",
            url,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run curl: {e}"))?;

    let mut body = Vec::new();
    let mut capped = false;
    if let Some(mut out) = child.stdout.take() {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match out.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    body.extend_from_slice(&buf[..n]);
                    if body.len() > MAX_BODY {
                        body.truncate(MAX_BODY);
                        capped = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    if capped {
        let _ = child.kill();
    }
    let exit = child.wait().map_err(|e| format!("curl wait failed: {e}"))?;
    let mut headers = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut headers);
    }
    let status = headers
        .lines()
        .filter(|l| l.starts_with("HTTP/"))
        .last()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse::<u16>().ok());
    if !exit.success() && !capped {
        let detail = headers
            .lines()
            .filter(|l| l.starts_with("curl:"))
            .last()
            .unwrap_or("unknown error")
            .trim()
            .to_string();
        return Err(format!("fetch failed: {detail}"));
    }
    Ok(Fetch { status, body })
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(';');
        let rep = match end {
            Some(e) if e <= 10 => match &tail[1..e] {
                "amp" => Some("&".into()),
                "lt" => Some("<".into()),
                "gt" => Some(">".into()),
                "quot" => Some("\"".into()),
                "apos" | "#39" => Some("'".into()),
                "nbsp" => Some(" ".into()),
                num if num.starts_with("#x") || num.starts_with("#X") => {
                    u32::from_str_radix(&num[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .map(|c| c.to_string())
                }
                num if num.starts_with('#') => num[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string()),
                _ => None,
            },
            _ => None,
        };
        match rep {
            Some(r) => {
                out.push_str(&r);
                rest = &tail[end.unwrap() + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Tag at byte `i` → (lowercase name, attr source, index after '>').
fn tag_at(html: &[u8], i: usize) -> Option<(String, String, usize)> {
    if html.get(i) != Some(&b'<') {
        return None;
    }
    let mut j = i + 1;
    let mut name = String::new();
    while j < html.len() && (html[j] as char).is_ascii_alphanumeric() {
        name.push((html[j] as char).to_ascii_lowercase());
        j += 1;
    }
    if name.is_empty() {
        return None;
    }
    // find '>' outside quotes; keep attr source
    let mut k = j;
    let mut quote = 0u8;
    let mut attr_src = Vec::new();
    while k < html.len() {
        let c = html[k];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            break;
        }
        attr_src.push(c);
        k += 1;
    }
    Some((name, String::from_utf8_lossy(&attr_src).to_string(), k + 1))
}

fn attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let lower = attrs.to_lowercase();
    let pat = format!("{key}=");
    let i = lower.find(&pat)? + pat.len();
    let rest = &attrs[i..];
    if let Some(r) = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\'')) {
        let q = attrs.as_bytes()[i] as char;
        r.find(q).map(|e| &r[..e])
    } else {
        rest.split(|c: char| c.is_whitespace() || c == '>')
            .next()
            .filter(|s| !s.is_empty())
    }
}

/// Position of `</name` (case-insensitive, word-boundaried) at/after `from`.
fn find_close(html: &[u8], from: usize, name: &str) -> Option<usize> {
    let nb = name.as_bytes();
    let mut i = from;
    while let Some(p) = html[i..].iter().position(|c| *c == b'<') {
        let j = i + p;
        let after = j + 2 + nb.len();
        if after > html.len() {
            return None;
        }
        if html[j + 1] == b'/'
            && html[j + 2..after].eq_ignore_ascii_case(nb)
            && matches!(
                html.get(after),
                Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t') | Some(b'\n')
            )
        {
            return Some(j);
        }
        i = j + 1;
    }
    None
}

/// Index just after the `</name…>` close tag (or end of input).
fn skip_block(html: &[u8], from: usize, name: &str) -> usize {
    match find_close(html, from, name) {
        Some(start) => html[start..]
            .iter()
            .position(|c| *c == b'>')
            .map(|e| start + e + 1)
            .unwrap_or(html.len()),
        None => html.len(),
    }
}

const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "ul",
    "ol",
    "tr",
    "td",
    "th",
    "table",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "nav",
    "blockquote",
    "pre",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
];
const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "template", "iframe", "head",
];

/// Readable text + (text, href) link list.
fn extract(html: &str) -> (String, Vec<(String, String)>) {
    let b = html.as_bytes();
    let mut text = String::new();
    let mut links = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            if b.get(i + 1) == Some(&b'!') {
                i = b[i..]
                    .iter()
                    .position(|c| *c == b'>')
                    .map(|e| i + e + 1)
                    .unwrap_or(b.len());
                continue;
            }
            if b.get(i + 1) == Some(&b'/') {
                // close tag: block-level closes end a paragraph line
                let mut j = i + 2;
                let mut cname = String::new();
                while j < b.len() && (b[j] as char).is_ascii_alphanumeric() {
                    cname.push((b[j] as char).to_ascii_lowercase());
                    j += 1;
                }
                if BLOCK_TAGS.contains(&cname.as_str()) {
                    text.push('\n');
                }
                i = b[j..]
                    .iter()
                    .position(|c| *c == b'>')
                    .map(|e| j + e + 1)
                    .unwrap_or(b.len());
                continue;
            }
            match tag_at(b, i) {
                Some((name, attrs, next)) => {
                    if SKIP_TAGS.contains(&name.as_str()) {
                        i = skip_block(b, next, &name);
                        continue;
                    }
                    if name == "a" {
                        let end = find_close(b, next, "a").unwrap_or(next);
                        let inner =
                            decode_entities(strip_tags(&html[next..end.min(html.len())]).trim());
                        if let Some(href) = attr(&attrs, "href") {
                            let href = decode_entities(href.trim());
                            if !href.is_empty() && !href.starts_with("javascript:") {
                                let label = if inner.is_empty() {
                                    href.clone()
                                } else {
                                    inner
                                };
                                text.push_str(&format!("[{label}]({href})"));
                                links.push((label, href));
                                i = skip_block(b, end, "a");
                                continue;
                            }
                        }
                        text.push_str(&inner);
                        i = skip_block(b, end, "a");
                        continue;
                    }
                    if BLOCK_TAGS.contains(&name.as_str()) {
                        text.push('\n');
                    }
                    i = next;
                    continue;
                }
                None => {
                    i += 1;
                    continue;
                }
            }
        }
        let end = b[i..]
            .iter()
            .position(|c| *c == b'<')
            .map(|p| i + p)
            .unwrap_or(b.len());
        text.push_str(&decode_entities(&html[i..end]));
        i = end;
    }
    (normalize(&text), links)
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank = false;
    for line in s.lines() {
        let mut collapsed = String::new();
        let mut ws = false;
        for c in line.trim().chars() {
            if c.is_whitespace() {
                ws = true;
            } else {
                if ws && !collapsed.is_empty() {
                    collapsed.push(' ');
                }
                ws = false;
                collapsed.push(c);
            }
        }
        if collapsed.is_empty() {
            blank = true;
        } else {
            if !out.is_empty() {
                out.push('\n');
                if blank {
                    out.push('\n');
                }
            }
            blank = false;
            out.push_str(&collapsed);
        }
    }
    out
}

/// Resolve `href` against `base`.
fn resolve_url(base: &str, href: &str) -> String {
    let href = href.trim();
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
        return href.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if let Some(h) = href.strip_prefix("//") {
        return format!("{scheme}://{h}");
    }
    if let Some(h) = href.strip_prefix('/') {
        return format!("{scheme}://{host}/{h}");
    }
    if href.starts_with('#') || href.is_empty() {
        return base.to_string();
    }
    let dir = if host_end == rest.len() {
        format!("{host}/")
    } else {
        rest[..rest.rfind('/').unwrap() + 1].to_string()
    };
    format!("{scheme}://{dir}{href}")
}

fn firecrawl(url: &str, key: &str) -> Result<String, String> {
    let payload = json!({ "url": url, "formats": ["markdown"] }).to_string();
    let out = std::process::Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "45",
            "-X",
            "POST",
            "https://api.firecrawl.dev/v1/scrape",
            "-H",
            &format!("Authorization: Bearer {key}"),
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    let body = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(format!(
            "firecrawl request failed: {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    let v: Value =
        serde_json::from_str(&body).map_err(|e| format!("firecrawl returned non-JSON: {e}"))?;
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        return Err(format!("firecrawl: {err}"));
    }
    v.get("data")
        .and_then(|d| d.get("markdown"))
        .or_else(|| v.get("markdown"))
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or_else(|| "firecrawl returned no markdown".to_string())
}

fn call_tool(name: &str, args: &Value) -> Result<String, String> {
    if name != "browse" {
        return Err(format!("unknown tool: {name}"));
    }
    let url = args.get("url").and_then(Value::as_str).unwrap_or("").trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("url must start with http:// or https://".into());
    }
    let action = args.get("action").and_then(Value::as_str).unwrap_or("read");
    if !["read", "links", "raw"].contains(&action) {
        return Err(format!("unknown action: {action} — read|links|raw"));
    }
    let provider = args
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let key = std::env::var("FIRECRAWL_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let use_firecrawl = match provider {
        "firecrawl" => {
            if key.is_none() {
                return Err("provider firecrawl needs FIRECRAWL_API_KEY".into());
            }
            true
        }
        "curl" => false,
        "auto" => key.is_some(),
        other => return Err(format!("unknown provider: {other} — auto|curl|firecrawl")),
    };
    if use_firecrawl {
        if action != "read" {
            return Err("firecrawl provider only supports action=read".into());
        }
        return firecrawl(url, key.as_deref().unwrap_or(""));
    }
    let page = fetch(url)?;
    if let Some(code) = page.status {
        if code >= 400 {
            return Err(format!("HTTP {code} from {url}"));
        }
    }
    let html = String::from_utf8_lossy(&page.body).to_string();
    let text = match action {
        "raw" => html.chars().take(RAW_CAP).collect::<String>(),
        "links" => {
            let (_, links) = extract(&html);
            let mut seen = std::collections::HashSet::new();
            let mut out = String::new();
            for (text, href) in links {
                let abs = resolve_url(url, &href);
                if seen.insert(abs.clone()) {
                    out.push_str(&format!("- [{text}]({abs})\n"));
                }
            }
            if out.is_empty() {
                "no links found".to_string()
            } else {
                out.trim_end().to_string()
            }
        }
        _ => {
            let (text, _) = extract(&html);
            if text.is_empty() {
                format!("(no readable text at {url})")
            } else {
                text
            }
        }
    };
    Ok(text)
}

/// One request → `Some(reply)`, or `None` for notifications. The bool asks
/// the loop to exit after writing the reply.
fn handle(req: &Value) -> (Option<Value>, bool) {
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        return (None, method == "plugin/shutdown");
    };
    let result = match method {
        "plugin/manifest" => manifest(),
        "tool/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("args").cloned().unwrap_or(Value::Null);
            match call_tool(name, &args) {
                Ok(text) => json!({ "content": text }),
                Err(text) => json!({ "content": text, "is_error": true }),
            }
        }
        "plugin/shutdown" => return (Some(json!({ "id": id, "result": {} })), true),
        _ => {
            let error = json!({ "code": -32601, "message": "method not found" });
            return (Some(json!({ "id": id, "error": error })), false);
        }
    };
    (Some(json!({ "id": id, "result": result })), false)
}

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("manifest") {
        println!("{}", manifest());
        return Ok(());
    }
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (reply, exit) = handle(&req);
        if let Some(reply) = reply {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
        if exit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, params: Value) -> Value {
        handle(&json!({ "id": 1, "method": method, "params": params }))
            .0
            .unwrap()
    }

    #[test]
    fn manifest_names_the_browse_tool() {
        let m = call("plugin/manifest", Value::Null)["result"].clone();
        assert_eq!(m["name"], "browse");
        assert_eq!(m["tools"][0]["name"], "browse");
        assert_eq!(m["tools"][0]["parameters"]["required"], json!(["url"]));
    }

    #[test]
    fn rejects_non_http_urls_and_bad_args() {
        let r = call(
            "tool/call",
            json!({ "name": "browse", "args": { "url": "ftp://x" } }),
        );
        assert_eq!(r["result"]["is_error"], true);
        let r = call(
            "tool/call",
            json!({ "name": "browse", "args": { "url": "https://a.b", "action": "nope" } }),
        );
        assert!(
            r["result"]["content"]
                .as_str()
                .unwrap()
                .contains("unknown action")
        );
        let r = call(
            "tool/call",
            json!({ "name": "browse", "args": { "url": "https://a.b", "provider": "firecrawl" } }),
        );
        assert!(
            r["result"]["content"]
                .as_str()
                .unwrap()
                .contains("FIRECRAWL_API_KEY")
        );
        let r = call("tool/call", json!({ "name": "browse", "args": {} }));
        assert_eq!(r["result"]["is_error"], true);
    }

    const PAGE: &str = r#"<!doctype html><html><head><title>T &amp; C</title>
        <style>body{color:red}</style><script>alert(1)</script></head>
        <body><h1>Hello   World</h1>
        <p>first&nbsp;para <a href="/about">About us</a> and <a href="https://x.example/y">ex</a></p>
        <p>second para</p>
        <a href="javascript:void(0)">dead</a>
        <div><a href="rel/page.html">rel</a></div>
        <script>more()</script></body></html>"#;

    #[test]
    fn extract_strips_scripts_and_keeps_links() {
        let (text, links) = extract(PAGE);
        assert!(!text.contains("alert"));
        assert!(!text.contains("color:red"));
        assert!(text.contains("Hello World"));
        assert!(text.contains("[About us](/about)"));
        assert!(text.contains("first para"));
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            decode_entities("a &amp; b &lt;x&gt; &#65; &#x42; &nbsp;z"),
            "a & b <x> A B  z"
        );
        assert_eq!(
            decode_entities("no &entities; &bogus"),
            "no &entities; &bogus"
        );
    }

    #[test]
    fn urls_resolve_against_base() {
        let base = "https://ex.com/a/b/page.html";
        assert_eq!(resolve_url(base, "/x"), "https://ex.com/x");
        assert_eq!(resolve_url(base, "y.html"), "https://ex.com/a/b/y.html");
        assert_eq!(resolve_url(base, "//cdn.com/z"), "https://cdn.com/z");
        assert_eq!(resolve_url(base, "https://o.com/"), "https://o.com/");
        assert_eq!(resolve_url("https://ex.com", "p"), "https://ex.com/p");
    }

    #[test]
    fn unknown_methods_are_method_not_found() {
        assert_eq!(call("nope", Value::Null)["error"]["code"], -32601);
    }

    #[test]
    fn shutdown_replies_then_exits() {
        let (reply, exit) = handle(&json!({ "id": 2, "method": "plugin/shutdown" }));
        assert!(reply.is_some() && exit);
    }
}

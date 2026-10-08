# gray-browse

Keyless web fetch: `read`/`links`/`raw` a URL via curl, with an optional
Firecrawl provider for JS-heavy pages. Port of the keyless half of hermes's
browser plugins (`browser/firecrawl`, `browser/browser_use`).

A sidecar plugin for [gray](https://github.com/vstaln/gray), scaffolded by
[gray-account](https://github.com/vstaln/gray-account).

## Tool

`browse` — `{url, action?, provider?}`

- `action:"read"` (default) — fetch with curl (follows redirects, 20s
  timeout, 8 MiB cap) and extract readable text in Rust: script/style/tags
  stripped, whitespace collapsed, links kept inline as `[text](href)`.
- `action:"links"` — only the link list, resolved against the page URL.
- `action:"raw"` — first 64 KiB of untouched HTML.
- `provider:"auto"` (default) — Firecrawl `/v1/scrape` when
  `FIRECRAWL_API_KEY` is set, else curl. `provider:"firecrawl"|"curl"`
  forces a path (firecrawl supports `read` only).

Errors are friendly: HTTP status codes and curl errors come back as tool
errors, never a panic.

## Wire methods

`plugin/manifest`, `plugin/shutdown`, `tool/call`. No hooks, no commands,
no host capabilities.

## Install

```sh
gray plugin install browse
```

## Develop

```sh
cargo test
gray account check      # entry point + manifest handshake
gray account publish    # check → build → release → publish to the gray registry
```

Bump `version` in `Cargo.toml` before each `publish`; the registry refuses to
republish a version.

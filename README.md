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

## Other tools

Ported from pi's `web-access` extension (MIT):

- `pdf_extract` — `{url_or_path, pages?}`: download or open a PDF, extract
  text with `pdftotext -f N -l N` (poppler) or `python3` + `pypdf`; without
  either it errors with an install hint. `pages` like `"1-5"`/`"3"`, output
  capped at 30k chars (head+tail).
- `youtube_transcript` — `{url}`: `yt-dlp --skip-download --write-subs
  --sub-langs en --convert-subs srt` into a tmpdir, then the `.srt` text;
  fallback scrapes `captionTracks` timedtext URLs from the watch page.
  Errors carry a yt-dlp install hint.
- `gh_clone` — `{repo, dest?}`: shallow `git clone --depth 1` of
  `owner/name` (or a git URL) into `~/.gray/browse/repos/<name>`; replies
  with the path and file count, `dest` overrides the target.

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

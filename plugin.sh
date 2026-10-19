#!/bin/sh
here="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
sys="$(uname -s)"
case "$sys" in
MINGW* | MSYS* | CYGWIN*) rel="x86_64-pc-windows-msvc/gray-browse.exe" ;;
Darwin*)
    case "$(uname -m)" in
    arm64) rel="aarch64-apple-darwin/gray-browse" ;;
    *) rel="x86_64-apple-darwin/gray-browse" ;;
    esac ;;
*)
    case "$(uname -m)" in
    aarch64 | arm64) rel="aarch64-unknown-linux-musl/gray-browse" ;;
    *) rel="x86_64-unknown-linux-musl/gray-browse" ;;
    esac ;;
esac
if [ -x "$here/$rel" ]; then exec "$here/$rel" "$@"; fi
for cand in "$here/target/release/gray-browse.exe" "$here/target/release/gray-browse" "$here/target/debug/gray-browse.exe" "$here/target/debug/gray-browse"; do
    if [ -x "$cand" ]; then exec "$cand" "$@"; fi
done
echo "gray-browse binary not found in $here" >&2
exit 1

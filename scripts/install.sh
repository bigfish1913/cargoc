#!/bin/sh
# Install only cargoc from a checksummed GitHub Release; never replaces cargo.
set -eu
repo=bigfish1913/cargoc
version= dir= dry=0
log() { printf '%s\n' "$*" >&2; }
die() { log "error: $*"; exit 1; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--dir)
      [ "$#" -ge 2 ] && [ -n "$2" ] || die "$1 requires a value"
      if [ "$1" = --version ]; then version=$2; else dir=$2; fi; shift 2 ;;
    --dry-run) dry=1; shift ;;
    -h|--help) log 'Usage: install.sh [--version X.Y.Z] [--dir PATH] [--dry-run]'; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done
case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux)
    os=unknown-linux-gnu
    # Do not install a glibc release onto musl/Alpine.
    if [ -f /etc/alpine-release ] || (ldd --version 2>&1 || true) | grep -qi musl; then die 'musl is not supported; compile with cargo install cargoc'; fi ;;
  *) die 'Use install.ps1 on Windows; supported Unix systems are Linux and macOS' ;;
esac
case "$(uname -m)" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; *) die 'unsupported CPU architecture' ;; esac
target=$arch-$os
if command -v curl >/dev/null 2>&1; then
  fetch() { curl --proto '=https' --proto-redir '=https' --connect-timeout 10 --max-time 120 -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget --https-only --timeout=60 -qO- "$1"; }
else die 'curl or wget is required'; fi
if [ -z "$version" ]; then
  version=$(fetch "https://api.github.com/repos/$repo/releases/latest" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\([^"]*\)".*/\1/p' | head -n1)
fi
version=${version#v}
case "$version" in ''|*[!0-9A-Za-z.+-]*) die 'invalid version; pass --version X.Y.Z' ;; esac
printf '%s\n' "$version" | LC_ALL=C grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$' || die 'invalid version; pass --version X.Y.Z'
[ -n "$dir" ] || dir=${HOME:?HOME is not set}/.local/bin
case "$dir" in /*) ;; *) dir=$(pwd)/$dir ;; esac
root=cargoc-v$version-$target
asset=$root.tar.gz
url=https://github.com/$repo/releases/download/v$version/$asset
log "platform: $target"; log "version: $version"; log "url: $url"; log "dir: $dir"
[ "$dry" -eq 0 ] || exit 0
[ ! -e "$dir/cargoc" ] && [ ! -L "$dir/cargoc" ] || die 'cargoc already exists here; use cargoc self-update or your package manager'
command -v tar >/dev/null 2>&1 || die 'tar is required'
if command -v sha256sum >/dev/null 2>&1; then
  hash_file() { sha256sum "$1" | cut -d ' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  hash_file() { shasum -a 256 "$1" | cut -d ' ' -f1; }
else die 'sha256sum or shasum is required'; fi
stage=
tmp=$(mktemp -d)
cleanup() { [ -z "$stage" ] || rm -rf "$stage"; rm -rf "$tmp"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
fetch "$url.sha256" > "$tmp/checksum" || die 'checksum download failed (required)'
# Validate the exact sidecar format without passing untrusted names to sha256sum -c.
[ "$(wc -l < "$tmp/checksum" | tr -d ' ')" -eq 1 ] || die 'invalid checksum file'
expected=$(LC_ALL=C sed -n 's/^\([0-9a-fA-F]\{64\}\)  \([^ ]*\)$/\1/p' "$tmp/checksum")
name=$(LC_ALL=C sed -n 's/^[0-9a-fA-F]\{64\}  \([^ ]*\)$/\1/p' "$tmp/checksum")
[ -n "$expected" ] && [ "$name" = "$asset" ] || die 'checksum names the wrong archive'
fetch "$url" > "$tmp/archive.tar.gz" || die 'archive download failed'
[ "$(hash_file "$tmp/archive.tar.gz" | tr A-F a-f)" = "$(printf '%s' "$expected" | tr A-F a-f)" ] || die 'SHA-256 verification failed'
# Stream only the exact member; never expand archive-controlled paths.
tar -xOzf "$tmp/archive.tar.gz" "$root/cargoc" > "$tmp/cargoc" || die 'expected binary is missing'
[ -s "$tmp/cargoc" ] || die 'empty executable'
chmod 700 "$tmp/cargoc"
[ "$("$tmp/cargoc" --version)" = "cargoc $version" ] || die 'binary version mismatch'
mkdir -p "$dir"
stage=$(mktemp -d "$dir/.cargoc-install.XXXXXX")
cp "$tmp/cargoc" "$stage/cargoc"; chmod 755 "$stage/cargoc"
# Link into the directory using the fixed source basename; even an existing
# destination directory is refused rather than treated as a new install root.
ln "$stage/cargoc" "$dir" || die 'destination appeared during installation; refusing to overwrite'
rm -rf "$stage"; stage=
log "Installed cargoc $version to $dir/cargoc"
case ":$PATH:" in *":$dir:"*) ;; *) log "Add this directory to PATH: $dir" ;; esac
log 'Optional: cargoc setup --enable (requires Cargo 1.91+)'

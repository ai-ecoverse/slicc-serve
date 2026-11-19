#!/bin/sh
set -eu

BIN=scserve
REPO=ai-ecoverse/slicc-serve

usage() {
  cat <<EOF
usage: install.sh [--version VERSION] [--dir DIR] [--help]

Installs $BIN from the $REPO release, checked against its SHA256SUMS.

  --version VERSION   the release to install (default: the latest)
  --dir DIR           where $BIN goes (default: ~/.local/bin)
  --help              print this usage and exit
EOF
}

fail() {
  printf 'install.sh: %s\n' "$1" >&2
  exit 1
}

usage_error() {
  printf 'install.sh: %s\n' "$1" >&2
  usage >&2
  exit 2
}

VERSION=
DIR=
while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || usage_error "--version needs a value"
      VERSION=$2
      shift 2
      ;;
    --version=*)
      VERSION=${1#*=}
      shift
      ;;
    --dir)
      [ $# -ge 2 ] || usage_error "--dir needs a value"
      DIR=$2
      shift 2
      ;;
    --dir=*)
      DIR=${1#*=}
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage_error "unknown option: $1"
      ;;
  esac
done

VERSION=${VERSION#v}
if [ -z "$DIR" ]; then
  [ -n "${HOME:-}" ] || fail "HOME is not set; give --dir"
  DIR=$HOME/.local/bin
fi

case "$(uname -s)" in
  Darwin) OS=darwin ;;
  Linux) OS=linux ;;
  MINGW* | MSYS* | CYGWIN*) fail "on Windows, use install.ps1: irm https://get.sliccy.ai/serve.ps1 | iex" ;;
  *) fail "no $BIN build for $(uname -s); use npm: npm install -g @ai-ecoverse/slicc-serve" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) ARCH=amd64 ;;
  arm64 | aarch64) ARCH=arm64 ;;
  *) fail "no $BIN build for $(uname -m)" ;;
esac

if [ -n "${SLICC_INSTALL_BASE_URL:-}" ]; then
  BASE=${SLICC_INSTALL_BASE_URL%/}
elif [ -n "$VERSION" ]; then
  BASE=https://github.com/$REPO/releases/download/v$VERSION
else
  BASE=https://github.com/$REPO/releases/latest/download
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsL -o "$2" "$1" || fail "could not download $1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1" 2>/dev/null || fail "could not download $1"
  else
    fail "neither curl nor wget is on PATH"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    fail "neither sha256sum nor shasum is on PATH"
  fi
}

ASSET=$BIN-$OS-$ARCH
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fetch "$BASE/SHA256SUMS" "$WORK/SHA256SUMS"
EXPECTED=$(awk -v name="$ASSET" '$2 == name || $2 == "*" name { print $1; exit }' "$WORK/SHA256SUMS")
[ -n "$EXPECTED" ] || fail "no checksum for $ASSET in $BASE/SHA256SUMS"
fetch "$BASE/$ASSET" "$WORK/$ASSET"
ACTUAL=$(sha256 "$WORK/$ASSET")
[ "$ACTUAL" = "$EXPECTED" ] || fail "checksum mismatch for $ASSET: expected $EXPECTED, got $ACTUAL"
chmod 755 "$WORK/$ASSET"

REPORTED=$("$WORK/$ASSET" --version 2>/dev/null) || fail "$ASSET does not run on this machine"
REPORTED=${REPORTED#"$BIN "}
if [ -n "$VERSION" ] && [ "$REPORTED" != "$VERSION" ]; then
  fail "$ASSET reports $REPORTED, not $VERSION"
fi

mkdir -p "$DIR" || fail "can't create $DIR"
TARGET=$DIR/$BIN
STAGED=$(mktemp "$DIR/.$BIN.XXXXXX") || fail "can't write to $DIR"
cp "$WORK/$ASSET" "$STAGED"
chmod 755 "$STAGED"
mv -f "$STAGED" "$TARGET" || fail "can't replace $TARGET"

printf '%s %s installed to %s\n' "$BIN" "$REPORTED" "$TARGET"
case ":${PATH:-}:" in
  *":$DIR:"*) ;;
  *) printf 'Add %s to your PATH:\n  export PATH="%s:%s"\n' "$DIR" "$DIR" "\$PATH" ;;
esac

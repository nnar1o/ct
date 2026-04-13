#!/usr/bin/env sh
set -eu

REPO="${CT_REPO:-nnar1o/ct}"
BIN_DIR="${CT_INSTALL_DIR:-$HOME/.local/bin}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'install.sh: missing required command: %s\n' "$1" >&2
    exit 1
  fi
}

normalize_tag() {
  case "$1" in
    v*) printf '%s' "$1" ;;
    *) printf 'v%s' "$1" ;;
  esac
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os:$arch" in
    Linux:x86_64|Linux:amd64) printf 'x86_64-unknown-linux-gnu' ;;
    Darwin:aarch64|Darwin:arm64) printf 'aarch64-apple-darwin' ;;
    *)
      printf 'install.sh: unsupported platform: %s/%s\n' "$os" "$arch" >&2
      printf 'install.sh: supported: Linux x86_64, macOS arm64\n' >&2
      exit 1
      ;;
  esac
}

latest_tag() {
  api_url="https://api.github.com/repos/$REPO/releases/latest"
  auth_header=""
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    auth_header="Authorization: Bearer ${GITHUB_TOKEN}"
  fi

  if [ -n "$auth_header" ]; then
    json="$(curl -fsSL -H "$auth_header" -H 'Accept: application/vnd.github+json' "$api_url")"
  else
    json="$(curl -fsSL -H 'Accept: application/vnd.github+json' "$api_url")"
  fi

  tag="$(printf '%s\n' "$json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | sed -n '1p')"
  if [ -z "$tag" ]; then
    printf 'install.sh: could not resolve latest release tag for %s\n' "$REPO" >&2
    exit 1
  fi
  printf '%s' "$tag"
}

need_cmd curl
need_cmd tar
need_cmd sed
need_cmd install
need_cmd mktemp
need_cmd uname

target="$(detect_target)"
tag="${CT_VERSION:-}"

if [ -n "$tag" ]; then
  tag="$(normalize_tag "$tag")"
else
  tag="$(latest_tag)"
fi

version="${tag#v}"
asset="ct-${version}-${target}.tar.gz"
url="https://github.com/$REPO/releases/download/$tag/$asset"

tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

printf 'Installing ct %s for %s\n' "$tag" "$target"
curl -fL --retry 3 --retry-delay 1 "$url" -o "$tmp_dir/$asset"

tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
package_dir="$tmp_dir/ct-${version}-${target}"

if [ ! -d "$package_dir" ]; then
  printf 'install.sh: archive content not found: %s\n' "$package_dir" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
for bin in ct ct-ctl ct-install; do
  if [ ! -f "$package_dir/$bin" ]; then
    printf 'install.sh: missing binary in archive: %s\n' "$bin" >&2
    exit 1
  fi
  install -m 0755 "$package_dir/$bin" "$BIN_DIR/$bin"
done

printf 'Installed binaries to %s\n' "$BIN_DIR"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) printf 'Add to PATH: export PATH="%s:$PATH"\n' "$BIN_DIR" ;;
esac

printf 'Next step: run ct-install to enable shell integration.\n'

#!/bin/sh
# Install a release of jlog, with no Rust: the program and its plugins.
#
#   sh install.sh [--tag vX.Y.Z] [--dir DIR]
#
# It maps `uname -s` and `uname -m` to a target, downloads jlog-<target>.tar.gz and its .sha256 from the GitHub
# release, and checks the hash. A hash that does not match stops the install. It puts jlog into DIR, which is
# ~/.local/bin by default, and the plugins into JLOG_PLUGINS, which is ~/.local/share/jlog/plugins by default, as
# `just install` does. Then it makes the environment of each plugin with uv, when uv is there, and prints the version.
# With no --tag, it installs the latest release. It needs only curl, tar, and sha256sum or shasum.
#
# The tests point it at a fake release with JLOG_RELEASE_BASE and JLOG_RELEASE_API, which can be file:// URLs.
set -eu

repo=rahuldave/jlog
base=${JLOG_RELEASE_BASE:-https://github.com/$repo/releases/download}
api=${JLOG_RELEASE_API:-https://api.github.com/repos/$repo/releases/latest}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

usage() {
  echo "usage: sh install.sh [--tag vX.Y.Z] [--dir DIR]"
}

tag=
dir=
while [ $# -gt 0 ]; do
  case $1 in
    --tag) [ $# -ge 2 ] || fail "--tag needs a value"; tag=$2; shift 2 ;;
    --tag=*) tag=${1#--tag=}; shift ;;
    --dir) [ $# -ge 2 ] || fail "--dir needs a value"; dir=$2; shift 2 ;;
    --dir=*) dir=${1#--dir=}; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; fail "unknown argument: $1" ;;
  esac
done

home=${HOME:-}
if [ -z "$dir" ]; then
  [ -n "$home" ] || fail "HOME is unset, so give --dir"
  dir=$home/.local/bin
fi
plugins=${JLOG_PLUGINS:-}
if [ -z "$plugins" ]; then
  [ -n "$home" ] || fail "HOME is unset, so set JLOG_PLUGINS"
  plugins=$home/.local/share/jlog/plugins
fi
case $plugins in
  /*) ;;
  *) fail "the plugins folder must be an absolute path: $plugins" ;;
esac

# The target of this machine. The Linux builds use musl, so they run on any distribution.
os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
  Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
  Darwin/x86_64)
    # A shell under Rosetta says x86_64 on an Apple Silicon Mac. The arm64 build runs there without emulation.
    if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
      target=aarch64-apple-darwin
    else
      target=x86_64-apple-darwin
    fi
    ;;
  Linux/x86_64|Linux/amd64) target=x86_64-unknown-linux-musl ;;
  Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl ;;
  *) fail "no release build for $os $arch. Build from source: cargo install --git https://github.com/$repo --tag <tag>" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is not in the PATH"
command -v tar >/dev/null 2>&1 || fail "tar is not in the PATH"

if [ -z "$tag" ]; then
  tag=$(curl -fsSL "$api" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1) ||
    fail "cannot read the latest release from $api"
  [ -n "$tag" ] || fail "cannot find the latest release in $api"
fi
case $tag in
  *"
"*) fail "not a release tag vX.Y.Z: the tag holds a newline" ;;
esac
echo "$tag" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' || fail "not a release tag vX.Y.Z: $tag"

tmp=$(mktemp -d "${TMPDIR:-/tmp}/jlog-install.XXXXXX") || fail "cannot make a temporary folder"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

asset=jlog-$target.tar.gz
echo "Downloading $asset of $tag"
curl -fsSL -o "$tmp/$asset" "$base/$tag/$asset" || fail "cannot download $base/$tag/$asset"
curl -fsSL -o "$tmp/$asset.sha256" "$base/$tag/$asset.sha256" || fail "cannot download $base/$tag/$asset.sha256"

# The hash check fails closed: a missing tool, an empty file or another hash stops the install.
want=$(awk 'NR == 1 { print $1 }' "$tmp/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$tmp/$asset" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  got=$(shasum -a 256 "$tmp/$asset" | awk '{ print $1 }')
else
  fail "neither sha256sum nor shasum is in the PATH, so the download cannot be checked"
fi
echo "$want" | grep -Eq '^[0-9a-f]{64}$' || fail "$asset.sha256 holds no SHA-256 hash"
[ "$got" = "$want" ] || fail "the SHA-256 of $asset is $got, but $asset.sha256 says $want"

mkdir "$tmp/unpacked"
tar -xzf "$tmp/$asset" -C "$tmp/unpacked" || fail "cannot unpack $asset"
[ -f "$tmp/unpacked/jlog" ] || fail "$asset holds no jlog"

# Copy, then rename, so a jlog that runs now keeps its old file.
mkdir -p "$dir" || fail "cannot make $dir"
for f in pyproject.toml uv.lock session.py; do
  for d in "$tmp/unpacked/plugins"/*/; do
    [ ! -d "$d" ] || [ -f "$d/$f" ] || fail "$asset holds a plugin without $f: $d"
  done
done
new=$dir/.jlog.new.$$
trap 'rm -rf "$tmp"; rm -f "$new"' EXIT
cp "$tmp/unpacked/jlog" "$new" || fail "cannot write in $dir"
chmod 755 "$new" || fail "cannot make $new executable"
mv -f "$new" "$dir/jlog" || fail "cannot put jlog in $dir"

for d in "$tmp/unpacked/plugins"/*/; do
  [ -d "$d" ] || continue
  n=$(basename "$d")
  mkdir -p "$plugins/$n" || fail "cannot make $plugins/$n"
  cp "$d/pyproject.toml" "$d/uv.lock" "$d/session.py" "$plugins/$n/" || fail "cannot copy the plugin $n to $plugins/$n"
done

# jlog removes these variables when it runs a plugin, so the sync does too, and both find the same environment.
if command -v uv >/dev/null 2>&1; then
  for v in $(env | sed -n 's/^\(UV_[A-Za-z0-9_]*\)=.*/\1/p; s/^\(PYTHON[A-Za-z0-9_]*\)=.*/\1/p'); do
    unset "$v"
  done
  unset VIRTUAL_ENV
  for d in "$plugins"/*/; do
    [ -f "$d/pyproject.toml" ] || continue
    uv sync --quiet --frozen --project "$d" || fail "uv sync failed for the plugin in $d"
  done
else
  echo "install.sh: uv is not in the PATH, so the plugins have no environment yet. Install uv, then run:" >&2
  echo "  for d in \"$plugins\"/*/; do uv sync --frozen --project \"\$d\"; done" >&2
fi

echo "Installed $("$dir/jlog" --version) in $dir, and the plugins in $plugins"
case ":${PATH:-}:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH." ;;
esac

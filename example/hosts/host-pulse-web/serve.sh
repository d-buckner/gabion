#!/usr/bin/env bash
# Build the pulse web host, generate the wasm-bindgen glue, and serve it.
#
#   ./example/hosts/host-pulse-web/serve.sh            # debug build, port 3458
#   PULSE_PROFILE=release ./.../serve.sh               # smaller, slower to build
#   PULSE_PORT=8080 ./.../serve.sh
#
# The wasm-bindgen CLI must match the crate version in Cargo.lock exactly, or
# the generated glue refuses to load. If the one on PATH does not match, this
# script downloads the matching release into target/ instead of asking you to
# reinstall your toolchain.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
profile="${PULSE_PROFILE:-debug}"
port="${PULSE_PORT:-3458}"

cargo_flags=()
wasm="$root/target/wasm32-unknown-unknown/$profile/host_pulse_web.wasm"
if [ "$profile" = "release" ]; then
  cargo_flags+=(--release)
fi

want="$(awk '/^name = "wasm-bindgen"$/{found=1} found && /^version = /{gsub(/"/,"",$3); print $3; exit}' "$root/Cargo.lock")"
if [ -z "$want" ]; then
  echo "cannot read the wasm-bindgen version from Cargo.lock" >&2
  exit 1
fi

bindgen="$(command -v wasm-bindgen || true)"
if [ -z "$bindgen" ] || ! "$bindgen" --version 2>/dev/null | grep -q " $want\$"; then
  tool="$root/target/wasm-bindgen-cli/$want/wasm-bindgen"
  if [ ! -x "$tool" ]; then
    case "$(uname -s)-$(uname -m)" in
      Darwin-arm64) triple=aarch64-apple-darwin ;;
      Darwin-x86_64) triple=x86_64-apple-darwin ;;
      Linux-x86_64) triple=x86_64-unknown-linux-musl ;;
      Linux-aarch64) triple=aarch64-unknown-linux-musl ;;
      *)
        echo "no prebuilt wasm-bindgen for $(uname -s)-$(uname -m);" >&2
        echo "install it with: cargo install wasm-bindgen-cli --version $want" >&2
        exit 1
        ;;
    esac
    echo "fetching wasm-bindgen $want ($triple) into target/…"
    mkdir -p "$(dirname "$tool")"
    curl -fsSL \
      "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/$want/wasm-bindgen-$want-$triple.tar.gz" \
      | tar xz -C "$(dirname "$tool")" --strip-components=1
  fi
  bindgen="$tool"
fi

echo "building host-pulse-web for wasm32-unknown-unknown ($profile)…"
cargo build -p host-pulse-web --target wasm32-unknown-unknown "${cargo_flags[@]}" \
  --manifest-path "$root/Cargo.toml"

dist="$here/dist"
rm -rf "$dist"
mkdir -p "$dist/pkg"
cp "$here/index.html" "$dist/index.html"
"$bindgen" --target web --no-typescript \
  --out-dir "$dist/pkg" --out-name pulse_web "$wasm"

echo "pulse web: http://127.0.0.1:$port  (ctrl-c to stop)"
exec python3 -m http.server "$port" --bind 127.0.0.1 --directory "$dist"

#!/usr/bin/env sh
set -eu

PROFILE="release"
PROFILE_FLAG="--release"

if [ "${1:-}" = "--debug" ]; then
  PROFILE="debug"
  PROFILE_FLAG=""
elif [ "${1:-}" = "--release" ] || [ -z "${1:-}" ]; then
  :
else
  echo "usage: scripts/build-wasm.sh [--release|--debug]" >&2
  exit 2
fi

if ! command -v wasm-bindgen >/dev/null 2>&1; then
  echo "wasm-bindgen-cli is required (use the version recorded in Cargo.lock)" >&2
  exit 1
fi

echo "Building pooya-portfolio for wasm32-unknown-unknown ($PROFILE)"
cargo build --locked --target wasm32-unknown-unknown $PROFILE_FLAG --lib

mkdir -p pkg
wasm-bindgen \
  --target web \
  --out-dir pkg \
  --out-name pooya_portfolio \
  "target/wasm32-unknown-unknown/$PROFILE/pooya_portfolio.wasm"

echo "Generated pkg/pooya_portfolio.js and pkg/pooya_portfolio_bg.wasm"


#!/usr/bin/env sh
set -eu

portfolio_root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$portfolio_root"

if [ "$#" -gt 1 ]; then
  echo "usage: ./build.sh [--release|--debug]" >&2
  exit 2
fi

portfolio_profile="${1:---release}"
case "$portfolio_profile" in
  --release|--debug)
    ;;
  *)
    echo "usage: ./build.sh [--release|--debug]" >&2
    exit 2
    ;;
esac

echo "Generating SEO timelines from data/timeline.json"
npm run build:timeline
npm run check:timeline

echo "Building the TypeScript bridge"
npm run build

echo "Building the Rust/WebGPU renderer"
scripts/build-wasm.sh "$portfolio_profile"

echo "Site build completed"

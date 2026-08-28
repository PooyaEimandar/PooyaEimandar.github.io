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

# Keep all browser build artifacts out of the maintained source tree.
rm -rf "$portfolio_root/_site"

echo "Building the TypeScript bridge"
npm run build

echo "Building the Rust/WebGPU renderer"
scripts/build-wasm.sh "$portfolio_profile"

echo "Staging the deployable site"
mkdir -p _site/data _site/timeline _site/assets/css _site/assets/fonts _site/assets/textures
cp CNAME THIRD_PARTY_NOTICES.md LICENSE robots.txt sitemap.xml _site/
portfolio_build_id="${GITHUB_SHA:-$(date -u +%Y%m%d%H%M%S)}"
sed "s/__BUILD_ID__/${portfolio_build_id}/g" index.html > _site/index.html
cp timeline/index.html _site/timeline/
cp assets/css/site.css _site/assets/css/
cp data/timeline.json _site/data/
cp assets/textures/pooya.ktx _site/assets/textures/
cp assets/fonts/FiraMono-LICENSE.txt assets/fonts/Vazirmatn-LICENSE.txt _site/assets/fonts/
touch _site/.nojekyll
if find _site -type f \( -iname '*.jpeg' -o -iname '*.jpg' -o -iname '*.gltf' -o -iname '*.bin' -o -iname '*.wgsl' -o -iname '*.ttf' -o -iname '*.ts' \) -print -quit | grep -q .; then
  echo "Source files and embedded renderer inputs must not be deployed." >&2
  exit 1
fi

echo "Site build completed in _site/"

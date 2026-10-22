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

# CI runs `npm run check:timeline` before this script, so HTML that was
# committed stale fails there instead of being regenerated here unnoticed.
echo "Generating SEO timelines from data/timeline.json"
npm run build:timeline

# Keep all browser build artifacts out of the maintained source tree.
rm -rf "$portfolio_root/_site"

echo "Building the TypeScript bridge"
npm run build

echo "Building the Rust/WebGPU renderer"
scripts/build-wasm.sh "$portfolio_profile"

echo "Staging the deployable site"
mkdir -p _site/data _site/timeline _site/assets/css _site/assets/fonts _site/assets/images
cp CNAME THIRD_PARTY_NOTICES.md LICENSE robots.txt 404.html _site/
portfolio_build_id="${GITHUB_SHA:-$(date -u +%Y%m%d%H%M%S)}"
sed "s/__BUILD_ID__/${portfolio_build_id}/g" index.html > _site/index.html
cp timeline/index.html _site/timeline/
cp assets/css/site.css _site/assets/css/
cp assets/images/social.png _site/assets/images/
cp data/timeline.json _site/data/
cp assets/fonts/FiraMono-LICENSE.txt assets/fonts/Vazirmatn-LICENSE.txt _site/assets/fonts/
touch _site/.nojekyll

# sitemap.xml is an index of every sitemap on pooya.ai, so Search Console needs
# only that one address. A section published from another repository adds its
# sitemap to section_sitemaps. Each lastmod is the date of the last commit that
# changed the page, so it is left out when there is no history.
section_sitemaps="https://pooya.ai/notes/sitemap.xml https://pooya.ai/webgpu/sitemap.xml"

sitemap_url() {
  page_url="$1"
  shift
  page_lastmod=$(git log -1 --format=%cs -- "$@" 2>/dev/null || true)
  printf '  <url>\n    <loc>%s</loc>\n' "$page_url"
  if [ -n "$page_lastmod" ]; then
    printf '    <lastmod>%s</lastmod>\n' "$page_lastmod"
  fi
  printf '  </url>\n'
}
{
  printf '<?xml version="1.0" encoding="UTF-8"?>\n'
  printf '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
  sitemap_url "https://pooya.ai/" index.html data/timeline.json
  sitemap_url "https://pooya.ai/timeline/" timeline/index.html data/timeline.json
  printf '</urlset>\n'
} > _site/sitemap-pages.xml
{
  printf '<?xml version="1.0" encoding="UTF-8"?>\n'
  printf '<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
  for section_sitemap in https://pooya.ai/sitemap-pages.xml $section_sitemaps; do
    printf '  <sitemap>\n    <loc>%s</loc>\n  </sitemap>\n' "$section_sitemap"
  done
  printf '</sitemapindex>\n'
} > _site/sitemap.xml
if find _site -type f \( -iname '*.jpeg' -o -iname '*.jpg' -o -iname '*.gltf' -o -iname '*.bin' -o -iname '*.wgsl' -o -iname '*.ttf' -o -iname '*.ktx' -o -iname '*.ts' \) -print -quit | grep -q .; then
  echo "Source files and embedded renderer inputs must not be deployed." >&2
  exit 1
fi

echo "Site build completed in _site/"

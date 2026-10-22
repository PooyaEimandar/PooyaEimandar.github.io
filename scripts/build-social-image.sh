#!/usr/bin/env bash
# Renders assets/images/social.png, the 1200x630 card that links to pooya.ai
# show when shared: a screenshot of the WebGPU scene with Pooya's name over it.
# PNG, because the build refuses JPEGs.
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd -- "$script_dir/.." && pwd)"
background="$project_dir/assets/images/social-background.webp"
font_path="$project_dir/assets/fonts/FiraMono-Regular.ttf"
output_path="${1:-$project_dir/assets/images/social.png}"

if ! command -v magick >/dev/null 2>&1; then
  echo "ImageMagick (magick) is required to build the social image" >&2
  exit 1
fi

# Slightly wider than the card, then cropped from the top centre, so the scene's
# copyright line at the bottom of the screenshot falls outside it. The text
# column on the left gets a dark wash so it reads over the falling glyphs.
magick "$background" -resize 1240x -gravity north -crop 1200x630+0+0 +repage \
  \( -size 760x630 -define gradient:direction=east \
     gradient:"rgba(1,4,2,0.9)-rgba(1,4,2,0)" \) \
  -gravity northwest -geometry +0+0 -compose over -composite \
  -font "$font_path" -fill "#56ff8f" -pointsize 28 -annotate +72+222 'pooya@portfolio:~$' \
  -fill "#e8f4ec" -pointsize 64 -annotate +68+302 'Pooya Eimandar' \
  -fill "#56ff8f" -pointsize 28 -annotate +72+380 'Technology entrepreneur' \
  -annotate +72+420 '& graphics engineer' \
  -fill "#c3ffd5" -pointsize 26 -annotate +72+500 'pooya.ai' \
  -strip -depth 8 -colors 256 -define png:compression-level=9 "$output_path"

echo "Generated $output_path (1200x630)"

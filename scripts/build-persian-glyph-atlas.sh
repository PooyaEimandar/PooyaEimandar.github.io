#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_dir="$(cd -- "$script_dir/.." && pwd)"
font_path="$project_dir/assets/fonts/Vazirmatn-Regular.ttf"
output_path="${1:-$project_dir/assets/textures/vazirmatn-persian.ktx}"

for command_name in magick toktx; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    echo "$command_name is required to build the Persian glyph atlas" >&2
    exit 1
  fi
done

if [[ ! -f "$font_path" ]]; then
  echo "Vazirmatn font is missing: $font_path" >&2
  exit 1
fi

atlas_work_dir="$(mktemp -d "${TMPDIR:-/tmp}/pooya-persian-atlas.XXXXXX")"
cleanup() {
  if [[ -n "${atlas_work_dir:-}" && -d "$atlas_work_dir" ]]; then
    rm -rf -- "$atlas_work_dir"
  fi
}
trap cleanup EXIT

glyphs=(
  "ا" "ب" "پ" "ت" "ث" "ج" "چ" "ح"
  "خ" "د" "ذ" "ر" "ز" "ژ" "س" "ش"
  "ص" "ض" "ط" "ظ" "ع" "غ" "ف" "ق"
  "ک" "گ" "ل" "م" "ن" "و" "ه" "ی"
)

cell_size=32
point_size=28
for index in "${!glyphs[@]}"; do
  printf -v glyph_name "glyph-%02d.png" "$index"
  magick \
    -size "${cell_size}x${cell_size}" \
    xc:none \
    -alpha on \
    -font "$font_path" \
    -pointsize "$point_size" \
    -fill white \
    -stroke none \
    -gravity center \
    -annotate +0+1 "${glyphs[$index]}" \
    "$atlas_work_dir/$glyph_name"
done

atlas_png="$atlas_work_dir/atlas.png"
magick "$atlas_work_dir"/glyph-0[0-7].png +append "$atlas_work_dir/row-0.png"
magick "$atlas_work_dir"/glyph-0[8-9].png "$atlas_work_dir"/glyph-1[0-5].png +append "$atlas_work_dir/row-1.png"
magick "$atlas_work_dir"/glyph-1[6-9].png "$atlas_work_dir"/glyph-2[0-3].png +append "$atlas_work_dir/row-2.png"
magick "$atlas_work_dir"/glyph-2[4-9].png "$atlas_work_dir"/glyph-3[0-1].png +append "$atlas_work_dir/row-3.png"
magick "$atlas_work_dir"/row-[0-3].png -append "$atlas_png"

mkdir -p -- "$(dirname -- "$output_path")"
toktx \
  --nometadata \
  --2d \
  --target_type RGBA \
  --input_swizzle rrrg \
  --assign_oetf srgb \
  "$output_path" \
  "$atlas_png"

echo "Generated $output_path ($((cell_size * 8))x$((cell_size * 4)), Vazirmatn Regular)"

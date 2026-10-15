#!/usr/bin/env bash
# Regenerate the README screenshots in docs/screenshots/.
#
#   scripts/readme-screenshots.sh
#
# Renders the `readme_shots` scenes off screen (src/app/headless/readme_shots.rs,
# 1600×1000 points at 2×, over sample projects in a temporary folder), then
# shrinks each to 1600×1000 and stores it as a 256-color PNG. Needs a GPU
# wgpu can use and ImageMagick's `magick`.
set -euo pipefail
cd "$(dirname "$0")/.."

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
STRATA_README_SHOTS="$WORK" cargo test --release readme_shots -- --exact app::headless::readme_shots::readme_shots
mkdir -p docs/screenshots
for f in "$WORK"/*.ppm; do
  name=$(basename "${f%.ppm}")
  magick "$f" -filter Lanczos -resize 1600x1000 -dither None -colors 256 \
    -define png:compression-level=9 -define png:compression-filter=5 -strip \
    "PNG8:docs/screenshots/$name.png"
done
ls -l docs/screenshots

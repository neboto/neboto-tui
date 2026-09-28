#!/usr/bin/env bash
# Copy the recordings into a neboto.dev checkout, minus VHS's window bar.
#
# The tapes draw a window bar (traffic-light dots) because the README shows
# the files bare. The site wraps every recording in its own styled
# `.window-bar`, so its copies are cropped instead of re-recorded: same
# footage, one bar. VHS's bar is 30px; the 12px padding below it is kept.
# Its 8px rounded bottom corners sit inside the site's 10px `.window` radius.
#
# Usage (from the repo root, after `vhs demo/neboto.tape` and the scenes):
#   demo/site-media.sh ../neboto.dev
set -euo pipefail

site=${1:?usage: demo/site-media.sh <neboto.dev checkout>}
media="$site/static/media"
[ -d "$media" ] || { echo "no $media — is $site a neboto.dev checkout?" >&2; exit 1; }
mkdir -p "$media/scenes"

bar=30
crop="crop=iw:ih-$bar:0:$bar"

mp4() {
  ffmpeg -v error -y -i "$1" -vf "$crop" -c:v libx264 -crf 20 -preset slow \
    -pix_fmt yuv420p -movflags +faststart -an "$2"
}
gif() {
  ffmpeg -v error -y -i "$1" \
    -vf "$crop,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none" "$2"
}
png() {
  ffmpeg -v error -y -i "$1" -vf "$crop" "$2"
}

mp4 demo/neboto.mp4 "$media/demo.mp4"
gif demo/neboto.gif "$media/demo.gif"
png demo/poster.png "$media/poster.png"
png demo/screenshot.png "$media/screenshot.png"
for scene in follow-links who-changed lambda-tail stack-drift; do
  mp4 "demo/scenes/$scene.mp4" "$media/scenes/$scene.mp4"
done
echo "wrote $media (recordings cropped by ${bar}px; the page's <img> for screenshot.png says 1200×670)"

#!/usr/bin/env bash
# Regenerate the neboto brand images from the in-app banner art
# (src/ui/widgets/banner.rs) — same glyphs, AWS orange on the catppuccin-mocha
# base the demo recordings use. Needs ImageMagick 7 (`magick`) and the
# JetBrainsMono Nerd Font Bold face (any Nerd Font with the box-drawing set
# works; change FONT).
#
#   demo/brand/make.sh            # writes the four PNGs next to this script
#
# Outputs:
#   neboto-avatar.png          1024² — GitHub org / Google Workspace avatar
#                              (GitHub applies its own circular crop)
#   neboto-avatar-rounded.png  1024² — rounded corners + transparent outside,
#                              for places that don't mask (Slack, docs)
#   neboto-social.png          1280×640 — the wordmark alone
#   neboto-preview.png         1280×640 — GitHub social preview + the site's
#                              og:image: wordmark, tagline, demo screenshot
set -euo pipefail
cd "$(dirname "$0")"
FONT="${FONT:-$HOME/Library/Fonts/JetBrainsMonoNerdFont-Bold.ttf}"
ORANGE='#FF9900'   # theme::aws_orange()
BASE='#1e1e2e'     # catppuccin-mocha base
DIM='#6c7086'      # catppuccin-mocha overlay0 (the banner's dim tagline)
TEXT='#cdd6f4'     # catppuccin-mocha text
SUBTEXT='#a6adc8'  # catppuccin-mocha subtext0
SURFACE='#313244'  # catppuccin-mocha surface0 (the screenshot's frame)
REGULAR="${REGULAR:-${FONT/-Bold/-Regular}}"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

# The N column of BANNER_ART (first 10 cells of each row).
printf '%s\n' "███╗   ██╗" "████╗  ██║" "██╔██╗ ██║" "██║╚██╗██║" "██║ ╚████║" "╚═╝  ╚═══╝" > "$tmp/n.txt"
# The full wordmark.
printf '%s\n' \
  "███╗   ██╗███████╗██████╗  ██████╗ ████████╗ ██████╗ " \
  "████╗  ██║██╔════╝██╔══██╗██╔═══██╗╚══██╔══╝██╔═══██╗" \
  "██╔██╗ ██║█████╗  ██████╔╝██║   ██║   ██║   ██║   ██║" \
  "██║╚██╗██║██╔══╝  ██╔══██╗██║   ██║   ██║   ██║   ██║" \
  "██║ ╚████║███████╗██████╔╝╚██████╔╝   ██║   ╚██████╔╝" \
  "╚═╝  ╚═══╝╚══════╝╚═════╝  ╚═════╝    ╚═╝    ╚═════╝ " > "$tmp/word.txt"

# Negative interline spacing closes the gap between rows so the blocks join.
magick -background none -fill "$ORANGE" -font "$FONT" -pointsize 160 -interline-spacing -30 \
  label:@"$tmp/n.txt" -trim +repage -resize 640x640 "$tmp/n.png"
magick -size 1024x1024 xc:"$BASE" "$tmp/n.png" -gravity center -composite neboto-avatar.png

magick neboto-avatar.png \
  \( +clone -alpha extract -draw 'fill black polygon 0,0 0,180 180,0 fill white circle 180,180 180,0' \
     \( +clone -flip \) -compose Multiply -composite \( +clone -flop \) -compose Multiply -composite \) \
  -alpha off -compose CopyOpacity -composite neboto-avatar-rounded.png

magick -background none -fill "$ORANGE" -font "$FONT" -pointsize 100 -interline-spacing -18 \
  label:@"$tmp/word.txt" -trim +repage -resize 1040x "$tmp/word.png"
magick -background none -fill "$DIM" -font "$FONT" -pointsize 44 -style Italic \
  label:'AWS Terminal Interface' -trim +repage "$tmp/tag.png"
magick -size 1280x640 xc:"$BASE" \
  "$tmp/word.png" -gravity center -geometry +0-40 -composite \
  "$tmp/tag.png"  -gravity center -geometry +0+130 -composite neboto-social.png

# The preview: smaller wordmark + tagline over the demo still, which runs off
# the bottom edge. The still loses VHS's 30px window bar (as demo/site-media.sh
# does) and gets its own rounded corners and frame.
magick "$tmp/word.png" -resize 470x "$tmp/word-sm.png"
magick ../screenshot.png -crop 1200x670+0+30 +repage -resize 1000x \
  \( +clone -alpha extract -draw 'fill black polygon 0,0 0,12 12,0 fill white circle 12,12 12,0' \
     \( +clone -flip \) -compose Multiply -composite \( +clone -flop \) -compose Multiply -composite \) \
  -alpha off -compose CopyOpacity -composite "$tmp/shot.png"
magick -size 1280x640 xc:"$BASE" \
  \( -size 1032x600 xc:none -fill "$SURFACE" -draw 'roundrectangle 0,0 1031,599 14,14' \) -geometry +124+246 -composite \
  "$tmp/shot.png" -geometry +140+262 -composite \
  "$tmp/word-sm.png" -gravity north -geometry +0+36 -composite \
  -font "$FONT" -pointsize 30 -fill "$TEXT" -annotate +0+154 'A read-only terminal UI for AWS' \
  -font "$REGULAR" -pointsize 21 -fill "$SUBTEXT" -annotate +0+196 '70+ services  ·  one search  ·  safe to point at prod' \
  -depth 8 neboto-preview.png

magick identify neboto-avatar.png neboto-avatar-rounded.png neboto-social.png neboto-preview.png | awk '{print $1, $3}'

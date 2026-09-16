#!/usr/bin/env bash
# Regenerate the neboto brand images from the in-app banner art
# (src/ui/widgets/banner.rs) — same glyphs, AWS orange on the catppuccin-mocha
# base the demo recordings use. Needs ImageMagick 7 (`magick`) and the
# JetBrainsMono Nerd Font Bold face (any Nerd Font with the box-drawing set
# works; change FONT).
#
#   demo/brand/make.sh            # writes the three PNGs next to this script
#
# Outputs:
#   neboto-avatar.png          1024² — GitHub org / Google Workspace avatar
#                              (GitHub applies its own circular crop)
#   neboto-avatar-rounded.png  1024² — rounded corners + transparent outside,
#                              for places that don't mask (Slack, docs)
#   neboto-social.png          1280×640 — GitHub social preview / README hero
set -euo pipefail
cd "$(dirname "$0")"
FONT="${FONT:-$HOME/Library/Fonts/JetBrainsMonoNerdFont-Bold.ttf}"
ORANGE='#FF9900'   # theme::aws_orange()
BASE='#1e1e2e'     # catppuccin-mocha base
DIM='#6c7086'      # catppuccin-mocha overlay0 (the banner's dim tagline)
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

magick identify neboto-avatar.png neboto-avatar-rounded.png neboto-social.png | awk '{print $1, $3}'

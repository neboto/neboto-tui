# neboto hero — VHS tape (https://github.com/charmbracelet/vhs)
#
# The loop at the top of README.md and neboto.dev: a deploy that failed, from
# "something's wrong" to "here's why" in about 20s. orders-worker is half
# healthy (◐); its Deployments show revision 15 FAILED; W shows CI deployed
# it; the stopped task's log says DATABASE_URL is not set.
#
# Run from the repo root:
#   cargo build --release             # the tape runs ./target/release/neboto
#   brew install ttyd ffmpeg          # vhs drives ttyd; ffmpeg encodes
#   go install github.com/charmbracelet/vhs@v0.11.0   # then put ~/go/bin on PATH
#     (v0.12.0 prints "Creating …" and writes nothing — charmbracelet/vhs#787)
#   vhs demo/neboto.tape              # writes demo/neboto.gif + .mp4 + poster.png
#
# Data: `neboto --demo`, the made-up acme-prod account in src/demo/ — no AWS
# account, no emulator, the same data every take, timestamps relative to now.
# The shorter clips for single workflows live in demo/scenes/.
#
# Cursor arithmetic: Services lists storefront-web then orders-worker, so `j`
# selects orders-worker; the Tasks tab's `f` cycles all → running → stopped.
#
# Sizing: 1200×700 px at 14px — the split pane needs ≥ 110 cols to render list
# + detail side by side without truncating the section bar. A Nerd Font is
# required for the ⛓ ◐ → glyphs.

Output demo/neboto.gif
Output demo/neboto.mp4

Set Shell "zsh"
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 14
Set Width 1200
Set Height 700
Set Padding 12
Set Framerate 12
Set TypingSpeed 80ms
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set BorderRadius 8

# demo/neboto.toml pins theme and banner so the machine's real config can't
# leak into a take; --demo pins account, region and data.
Env NEBOTO_CONFIG "demo/neboto.toml"
# `e` opens the log in $EDITOR; less reads well on camera, and its prompt says
# how to get back instead of showing the temp file's path.
Env EDITOR "less"
Env LESS "-Psq returns to neboto"

# ── Boot off-camera ───────────────────────────────────────────────────────────
# CloudWatch first, so W can merge alarm history; then ECS on the Services tab.
Hide
Type "./target/release/neboto --demo -s cw"
Enter
Sleep 3s
Type "@ecs"
Sleep 300ms
Enter
Sleep 2.5s
Type "2"
Sleep 1s
Show

# The first frame doubles as the site's video poster.
Screenshot demo/poster.png
Sleep 1.5s

# ── 1. Something's wrong: orders-worker is only half healthy ──────────────────
Type "j"
Sleep 700ms
Enter
Sleep 1.2s

# Deployments: revision 15 FAILED, six tasks failed to start, rolled back.
Type "2"
Sleep 3s

# ── 2. What changed? CI registered revision 15 and deployed it ────────────────
Type "Z"
Sleep 500ms
Type "W"
Sleep 3.5s
Escape
Sleep 300ms
Type "Z"
Sleep 300ms
Escape
Sleep 500ms

# ── 3. Why did it fail? The stopped task's log ────────────────────────────────
Type "4"
Sleep 700ms
Type "ff"
Sleep 1s
Enter
Sleep 1.5s
Type "e"
Sleep 4.5s

# Back to neboto, then leave before the recording ends so ttyd doesn't hold
# the tty.
Type "q"
Sleep 800ms
Hide
Type "q"
Sleep 300ms

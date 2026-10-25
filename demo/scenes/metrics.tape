# Scene: the metrics view. orders-worker's CPU and memory, with the failed
# deploy drawn on the chart; W opens those changes, Esc comes back. Then
# orders-api's errors, which have been climbing for 40 minutes, and its
# latency peaks over 30 days.
#
# Rows counted:
#   - ECS Services: storefront-web, then orders-worker → j
#   - Lambda metrics: Invocations, Errors, Throttles, Duration → j, then j j

Output demo/scenes/metrics.gif
Output demo/scenes/metrics.mp4

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

# demo/neboto.toml pins theme and banner; --demo pins account, region and data.
Env NEBOTO_CONFIG "demo/neboto.toml"

# ── Boot off-camera ──────────────────────────────────────────────────────────
Hide
# Load CloudWatch first, so the timeline (W) can merge alarm history instead
# of noting "@cloudwatch not loaded", then switch to the scene's service.
Type "./target/release/neboto --demo --show-keys -s cw"
Enter
Sleep 3s
Type "@ecs"
Sleep 300ms
Enter
Sleep 2.5s
Type "2"
Sleep 1s
Type "j"
# --show-keys: let the boot's keys fade (2.5s) before the camera rolls.
Sleep 2.8s
Show
Sleep 1s

# m charts the service; Z gives the chart the full width. The UpdateService
# and RegisterTaskDefinition lines 40 minutes ago are the failed deploy, and
# the chart's border names them.
Type "m"
Sleep 1s
Type "Z"
Sleep 3.5s
# Memory, one row down.
Type "j"
Sleep 2.5s

# W: those changes as the timeline. Esc returns to the charts as they were.
Type "W"
Sleep 4s
Escape
Sleep 2s
Escape
Sleep 500ms

# ── orders-api: errors climbing ──────────────────────────────────────────────
Type "@lambda"
Sleep 300ms
Enter
Sleep 2.5s
Type "m"
Sleep 1s
Type "Z"
Sleep 2s
# Errors: up since 40 minutes ago, and the alarm that fired is on the chart.
Type "j"
Sleep 3s
# Duration; s cycles its statistic (Average → Maximum → …), 5 widens to the
# last 30 days.
Type "jj"
Sleep 1.5s
Type "s"
Sleep 2s
Type "5"
Sleep 3.5s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms
Type "q"
Sleep 300ms

# Scene: a deploy that failed. orders-worker's revision 15 couldn't start;
# the circuit breaker rolled it back. Deployments → timeline → crash log.

Output demo/scenes/failed-deploy.gif
Output demo/scenes/failed-deploy.mp4

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
# `e` opens the log in $EDITOR; less reads well on camera and q returns.
Env EDITOR "less"

# ── Boot off-camera ──────────────────────────────────────────────────────────
Hide
Type "./target/release/neboto --demo -s ecs"
Enter
Sleep 3s
Show
Sleep 1s

# Services tab; orders-worker (◐, rollout in trouble) is the second row.
Type "2"
Sleep 1.2s
Type "j"
Sleep 600ms
Enter
Sleep 1.5s

# Deployments: revision 15 FAILED, 6 tasks failed to start.
Type "2"
Sleep 2.5s

# Timeline: the CI role registered revision 15 and deployed it, then ECS
# rolled back.
Type "Z"
Sleep 600ms
Type "W"
Sleep 3.5s
Escape
Sleep 400ms
Type "Z"
Sleep 400ms
Escape
Sleep 600ms

# Tasks tab, filtered to stopped: the two tasks from the failed revision.
Type "4"
Sleep 900ms
Type "ff"
Sleep 1.2s
Enter
Sleep 2s

# The container's log: DATABASE_URL is not set.
Type "e"
Sleep 4s
Type "q"
Sleep 1s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms

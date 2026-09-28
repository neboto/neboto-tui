# Scene: "who changed this?" web-sg has SSH open to 0.0.0.0/0. W merges
# CloudTrail (and alarms, stack events, deployments) for this one resource.

Output demo/scenes/who-changed.gif
Output demo/scenes/who-changed.mp4

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
Type "./target/release/neboto --demo -s cw"
Enter
Sleep 3s
Type "@ec2"
Sleep 300ms
Enter
Sleep 2.5s
Show
Sleep 1s

# Security Groups tab; web-sg is the first row. Open it: the inbound rules,
# with the open SSH rule in the warning colour.
Type "2"
Sleep 2s
Enter
Sleep 3.5s

# The change timeline: jsmith opened port 22 five days ago.
# Z: give the pane the full width so the lens columns fit.
Type "Z"
Sleep 600ms
Type "W"
Sleep 5s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms

# Scene: live-tail a Lambda. orders-api logs every invocation; about one in
# nine hits a DynamoDB throttle. / filters the tail to those.

Output demo/scenes/lambda-tail.gif
Output demo/scenes/lambda-tail.mp4

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
Type "./target/release/neboto --demo -s lambda"
Enter
Sleep 3s
Show
Sleep 1s

# orders-api is the first row. t starts the tail, seeded with the last 15m.
Sleep 1s
Type "t"
Sleep 5s

# Filter to the errors; new ones keep arriving.
Type "/"
Sleep 300ms
Type "ERROR"
Sleep 500ms
Enter
Sleep 5s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms

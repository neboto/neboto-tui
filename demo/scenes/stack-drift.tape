# Scene: stack drift. storefront-prod has drifted; the Drift section shows
# which property, and W on the drifted target group shows who changed it.
#
# Rows counted:
#   - Stack · 8 Drift: 0 blank, 1 Stack Drift, 2 blank, 3 WebTargetGroup,
#     4 Type, 5 Physical ID → jjjjj ⏎ follows the target group

Output demo/scenes/stack-drift.gif
Output demo/scenes/stack-drift.mp4

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
Type "@cfn"
Sleep 300ms
Enter
Sleep 2.5s
Show
Sleep 1s

# storefront-prod is the first row. Open it.
Enter
Sleep 2s

# Drift: the target group's health-check interval is 15, the template says 30.
Type "8"
Sleep 2.5s

# Follow the drifted resource, then ask who changed it.
Type "jjjjj"
Sleep 800ms
Enter
Sleep 2s
# Z: give the pane the full width so the lens columns fit.
Type "Z"
Sleep 600ms
Type "W"
Sleep 5s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms

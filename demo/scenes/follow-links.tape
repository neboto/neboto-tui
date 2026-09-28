# Scene: follow the links. ECS service → its target group → the ALB in
# front of it, then back, and N: the service's effective network access.
#
# Rows counted:
#   - ECS service · 4 Networking: 0 blank, 1 header, 2 Container, 3 Target Group → j j j ⏎
#   - Target group · 2 Config: G lands on the trailing blank row; k is the
#     storefront-alb row under "Load Balancers" → G k ⏎

Output demo/scenes/follow-links.gif
Output demo/scenes/follow-links.mp4

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
Type "@ecs"
Sleep 300ms
Enter
Sleep 2.5s
Show
Sleep 1s

# Services tab; storefront-web is the first row. Open it.
Type "2"
Sleep 1.2s
Enter
Sleep 1.5s

# Networking: the load balancer wiring, with → on everything followable.
Type "4"
Sleep 2s
Type "jjj"
Sleep 800ms
Enter
Sleep 2.2s

# The target group: three healthy targets. Its Config lists the ALB.
Type "2"
Sleep 1.5s
Type "G"
Sleep 300ms
Type "k"
Sleep 800ms
Enter
Sleep 2s

# The ALB's listeners: HTTPS forwards to the target group, HTTP redirects.
Type "2"
Sleep 2.5s

# Ctrl-O walks back through the jump history to the service we started on.
Ctrl+O
Sleep 700ms
Ctrl+O
Sleep 1.2s

# N: the service's effective inbound access. SSH is open to the world, and
# 8080 is only reachable from the ALB, which is labelled because its cache is
# now warm.
Type "Z"
Sleep 600ms
Type "N"
Sleep 5s

# Leave the app before the recording ends so ttyd does not hold the tty.
Hide
Type "q"
Sleep 300ms

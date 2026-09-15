# neboto README demo — VHS tape (https://github.com/charmbracelet/vhs)
#
# Renders the loop at the top of README.md: one instance, then "jump around" —
# instance → @vpc → subnet → (link) VPC → @ec2 security group → Used By →
# (link) instance → Z for the full-width pane. ~35s visible, loops.
#
# Run from the repo root:
#   cargo build --release             # the tape runs ./target/release/neboto
#   brew install ttyd ffmpeg          # vhs drives ttyd; ffmpeg encodes
#   go install github.com/charmbracelet/vhs@v0.11.0   # then put ~/go/bin on PATH
#     (v0.12.0 prints "Creating …" and writes nothing — charmbracelet/vhs#787)
#   vhs demo/neboto.tape              # writes demo/neboto.gif (+ .mp4 for reuse)
#
# Data: the local floci emulator, seeded by scripts/seed-floci.sh — nothing on
# screen is real, so nothing needs blurring. The tab bar shows a
# `⚙ localhost:4566` badge while an endpoint override is active; that is the
# honest signal it is an emulator, leave it.
#   - floci running on http://localhost:4566 (`floci start`, then the seeder).
#   - Instances are named neboto-seed-001 … -0NN; the seed VPC's one subnet is
#     10.0.1.0/24 (that CIDR is the stable handle — ids change on reseed); the
#     seed SG is named `neboto-seed` and carries three ingress rules (443 from
#     anywhere renders in the warning colour). Older seeds have no rules —
#     re-run the seeder or add them by hand.
#   - Every seeded instance's ENI is in the seed SG, so its Used By section
#     lists 40 interfaces, each row's value carrying the attached instance id.
#
# Cursor arithmetic (the detail cursor is a plain row index; blank spacer rows
# count, and every section switch resets it to row 0):
#   - Subnet · Details:  row 0 Subnet ID, row 1 VPC ID          → `j` ⏎
#   - SG · Used By:      row 0 blank, 1 header, 2 blank, 3 ENI  → `]]` `j` `j` ⏎
#     (the jump classifier reads the *value* first, and the value is
#     "i-… · ip · status", so ⏎ lands on the instance, not the ENI)
#
# Sizing: 1200×700 px at 16px ≈ 120×36 cells — the split pane needs ≥ 110 cols
# to render list + detail side by side without truncating the section bar.
# A Nerd Font is required for the ⛓ ⚠ ✓ → glyphs.

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

# All settings (service, region, theme, no banner) live in demo/neboto.toml so
# the machine's real config can't leak into a take and every scene tape shares
# one setup. Paths are relative to the directory `vhs` is run from (repo root).
Env NEBOTO_CONFIG "demo/neboto.toml"
# Point every SDK client at floci (env wins over any endpoint_url in config).
Env AWS_ENDPOINT_URL "http://localhost:4566"

# ── Boot off-camera: the list load is a spinner nobody needs to watch ──────────
Hide
Type "./target/release/neboto"
Enter
Sleep 4s
Show

Sleep 1.2s

# ── 1. Find one instance and open it ──────────────────────────────────────────
Type "/"
Sleep 300ms
Type "seed-031"
Sleep 900ms
# Enter closes the search bar but keeps the filter applied; a second Enter
# drills into the split detail pane.
Enter
Sleep 400ms
Enter
Sleep 1.8s

# A little detail: Security (its groups), then Networking (ENI / subnet / VPC).
Tab
Sleep 1.5s
Tab
Sleep 1.8s

# ── 2. Switch service from inside the pane: @vpc ──────────────────────────────
Type "@"
Sleep 300ms
Type "vpc"
Sleep 700ms
Enter
Sleep 1.5s

# Subnets sub-tab, filter to the seed subnet by CIDR, open it.
Type "2"
Sleep 900ms
Type "/"
Sleep 300ms
Type "10.0.1"
Sleep 800ms
Enter
Sleep 400ms
Enter
Sleep 1.8s

# ── 3. Follow the VPC ID link back to the VPC ─────────────────────────────────
Type "j"
Sleep 700ms
Enter
Sleep 1.8s
# The VPC's own Subnets section lists the subnet we just came from.
Type "2"
Sleep 1.8s

# ── 4. @ec2 → Security Groups tab → the seed group ────────────────────────────
Type "@"
Sleep 300ms
Type "ec2"
Sleep 700ms
Enter
Sleep 1.2s
Type "2"
Sleep 900ms
Type "/"
Sleep 300ms
Type "neboto-seed"
Sleep 800ms
Enter
Sleep 400ms
Enter
# Inbound rules: 22 / 80 internal, 443 open to the world in the warning colour.
Sleep 2.2s

# ── 5. Used By (lazy) → first interface → follow it to its instance ───────────
Type "3"
Sleep 1.6s
Type "]]"
Sleep 400ms
Type "j"
Sleep 250ms
Type "j"
Sleep 800ms
Enter
Sleep 2s

# ── 6. Z: give the detail pane the whole width, and end on that frame ─────────
Type "Z"
Sleep 3s

# Leave the app before the recording ends so ttyd doesn't hold the tty.
Hide
Type "q"
Sleep 300ms

# Showcase scenes

One short clip per workflow, recorded against `neboto --demo`: no AWS
account, no emulator, and the same data every take. Everything on screen is
the made-up `acme-prod` account from `src/demo/fixtures.rs`, whose header
lists what's planted in it.

The failed-deploy story is the hero loop, `demo/neboto.tape`, which also
writes the site's `demo/poster.png`. `who-changed.tape` writes the site's
still, `demo/screenshot.png`. The README shows the four below as a gallery.

| Scene | Shows |
|---|---|
| `follow-links.tape` | Enter follows links: ECS service → target group → ALB, back with `Ctrl-O`, then `N` shows the service's effective network access |
| `who-changed.tape` | `W` on `web-sg`: SSH open to the world, and CloudTrail says who opened it |
| `lambda-tail.tape` | `t` live-tails a Lambda's logs, `/` filters to the errors |
| `stack-drift.tape` | A CloudFormation stack that drifted, and `W` finds the console edit behind it |

Record from the repo root, with the tools the main tape lists (`vhs` v0.11.0,
`ttyd`, `ffmpeg`, a Nerd Font):

```bash
cargo build --release
vhs demo/neboto.tape                              # the hero
for t in demo/scenes/*.tape; do vhs "$t"; done   # writes demo/scenes/<name>.gif + .mp4
# (go install puts vhs in ~/go/bin, which may not be on PATH)
```

Each tape loads CloudWatch off-camera before switching to its service, so the
change timeline (`W`) can merge alarm history instead of noting that
CloudWatch isn't loaded. The hero's crash log sets `EDITOR=less` with a
`LESS` prompt, so the frame doesn't show a temp file path.

To update neboto.dev after re-recording, run `demo/site-media.sh
<neboto.dev checkout>`. It copies the files, cropping off VHS's window bar,
because the site frames each recording in its own.

Timestamps in the demo are relative to now, so a take never looks stale. The
live tail is generated per request (`src/demo/generate.rs`), so it keeps
moving for as long as the camera rolls.

Cursor arithmetic: the detail cursor is a plain row index, blank spacer rows
count, and a section switch resets it to row 0. Each tape notes the rows it
counts; if a section's layout changes, re-check those lines.

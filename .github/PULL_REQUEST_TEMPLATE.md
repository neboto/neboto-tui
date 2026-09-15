## What

<!-- one or two sentences: what changes and why -->

## Tested against

<!-- real account / floci or LocalStack (`scripts/seed-floci.sh`) / offline harness only -->

## Checklist

- [ ] `cargo test` and `cargo clippy` pass with no new warnings
- [ ] `python3 scripts/check-readonly.py` passes — **no mutating AWS calls** (allowlist additions explained above)
- [ ] New/changed AWS calls are in `PERMISSIONS.md`
- [ ] New service or sub-tab: registered in `App::build_services()`, harness mock added, `docs/SERVICES.md` entry written
- [ ] Visual change: screenshot or recording attached; colors go through `theme.rs`
- [ ] `README.md` / `docs/` updated if behaviour changed

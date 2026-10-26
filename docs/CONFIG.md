# Config options (`src/config.rs`)

Moved out of the root `CLAUDE.md` so it loads only when needed. `src/config.rs`
is the reference; this is the annotated list.

- **`src/config.rs`**: TOML `Config` (`$NEBOTO_CONFIG` → XDG → `~/.neboto.toml`):
  `default_service`/`default_region`/`default_profile`, `show_banner`,
  `endpoint_url`, `watch`/`watch_interval` (start in watch mode / its cadence),
  `detail_flat` (start in the flat all-section detail view),
  `log_wrap` (start log tail/search panes with long lines wrapped),
  `detail_wrap` (start the detail body with long values wrapped — `Ctrl-W`),
  `show_keys` (keycast: each key and what it did, in a corner box),
  `export_formats` (which of `json`/`csv`/`md` exports write, default all
  three — `export::ExportFormats`; bad values warn at startup, never fatal) +
  `export_dir` (`~/` expanded, created on demand; `NEBOTO_EXPORT_DIR` wins),
  `theme` (presets: `dark` default, `light`, `solarized-dark/-light`,
  `gruvbox-dark/-light`, `dracula`, `nord`, `catppuccin-mocha/-latte` —
  `theme::PRESET_NAMES`; separators optional, `mocha`/`latte` also resolve) +
  `[theme_colors]` (per-color overrides by palette field name, `#rrggbb` or
  ANSI names; bad keys/values warn at startup, never fatal),
  `org_access_role`/`org_access_roles` (member-account switch role name(s),
  default `OrganizationAccountAccessRole`; a list opens the role picker),
  `controltower_audit_account` (+ optional `controltower_audit_role`,
  defaulting to the member-switch role) — the account holding the Control
  Tower Config aggregator, so the Compliance tab works from the management
  account; **quote it**, a bare 12-digit id is a TOML integer and the whole
  file then fails to parse,
  `owner_tags` (owner-ish tag keys for the ownership ribbon, default
  `["owner", "team"]`, case-insensitive) + `managed_by_tags` (ManagedBy-marker
  keys, default `["managedby", "managed-by", "managed_by"]`) +
  `ownership_ribbon` (default true; `false` hides the ribbon, timeline
  unaffected),
  `update_check` (default true: a once-a-day anonymous GET of the latest
  GitHub Release, `src/update_check.rs` — the **only non-AWS request**, so it
  stays silent on failure, never runs in tests / headless / `--demo` / CI
  (`App::spawn_update_check` is called from `main.rs`'s TUI startup only),
  and shows as a clickable `↑ vX.Y.Z` service-strip chip),
  `cache_ttl` (base cache freshness in seconds, default 300) +
  `[cache_ttls]` (per-service overrides keyed by any `@`-search prefix;
  unknown prefixes are ignored),
  `pinned_services` (prefixes seeded into `visited_services` so their strip
  chips show before they load; unknown prefixes warn at startup),
  `regions` (your working set: `ls --regions` fans out over it, and the `R`
  picker lists it first and counts the current service in each; unknown
  codes warn at startup).
  No `default_service` ⇒ welcome splash, nothing loads.

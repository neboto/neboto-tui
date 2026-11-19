# Caching: refresh, watch mode, region fan-out

Moved out of the root `CLAUDE.md` so it loads only when needed. TTLs, the
global-service key and the variant key stayed there. Read this before
touching `refresh_*`, `watch_tick`, `watch_staging` or `start_region_fanout`.

- `r` in the **list pane** invalidates + reloads the whole service **and
  replaces the `LazyStore`** (epoch bump — `refresh_current_service`): without
  this, LazyMap's contains-guard served the first fetch of every lazy detail
  section (e.g. an ECR repo's Images) for the rest of the session. When the
  detail pane is focused it re-fires the open section's trigger before the
  reload clears the selection, so the pane refetches instead of spinning over
  the emptied store (and `flat_triggered_for` is reset so the flat view's
  sweep runs again).
- `r` in the **detail pane** = targeted refresh: `refresh_selected_resource`
  re-fetches just the selected resource and swaps it in place (by `id`, same
  position) via `Event::ResourceRefreshed`. Supported types downcast to a
  `fetch_single_*` helper (ECS service/task today; their lazy sections are
  NOT cleared on the targeted path); others fall back to full refresh
  (incl. the lazy-store replacement above). Doesn't touch the cache.
- **Watch mode** (`w`): auto-refresh of the current view every `WatchState`
  interval (default 10s; `+`/`-` presets 5–300s; config `watch`/
  `watch_interval`, CLI `--watch`). Driven by the existing 250ms `Event::Tick`
  → `watch_tick()`; held over while anything is loading / a modal, search, or
  overlay is up. List refreshes stream into `App.watch_staging` (the visible
  list **never blanks**) and swap in atomically on `ResourcesFullyLoaded`,
  selection restored by id; detail focus on an ECS service/task uses the
  targeted single-resource path with `quiet` toasts. Status bar shows a green
  `⟳ watch Ns` chip — `show_loading_indicator()` stays quiet for watch
  refreshes. `load_service_resources()` drops any in-flight staging (a manual
  load supersedes the watch pass).
  **Over the charts** (`m`) it refreshes them instead of holding off
  (`watch_tick_charts`): never faster than once a minute
  (`chart_watch_interval` — CloudWatch's resolution; the chip shows that
  cadence), timed off `metrics_window_end` so `r` / a range key resets it,
  and a dashboard pass is `quiet` — the grid stays drawn, and stays if the
  pass fails. Cost / S3 charts are daily and aren't refreshed.
- **A region switch keeps the cache** (it's region-keyed; only a profile/role
  switch replaces it). That's what makes the **`R` fan-out** (#92) pay off:
  opening the picker with config `regions` set lists the current service in
  each uncached configured region (`start_region_fanout`, on
  `AwsClients::for_region`, via the headless `list_all`) straight into that
  region's slot (`Event::RegionFanoutLoaded`, `region_fanout_generation` —
  bumped on profile/role switches only), and the picker shows
  `region_counts()`: rows of the active sub-tab's type per region. Skipped
  for global, virtual and **variant-cached** services — every service whose
  list depends on App state is variant-cached, and a fresh `build_services`
  would list it with the default.

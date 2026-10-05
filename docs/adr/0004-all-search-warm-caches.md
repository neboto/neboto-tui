# `@all` searches warm caches only, and never fetches

`@all <text>` searches every service's **already-cached** list and never
starts a load. A service you haven't visited (or whose cache has expired)
isn't searched. The service strip marks it `–` so that "no match" never reads
as "not searched".

## Considered options

- **Warm caches only (chosen).** It's instant, costs no API calls, and is
  honest about its coverage. That's useful exactly as scoped.
- **Background fan-out: load every service, then search.** It's slow. It
  invites throttling from the N+1 describes many services need. And in
  locked-down accounts it's permission-noisy: dozens of AccessDenied warnings
  for services the user never meant to touch. Scoped as "search everything",
  `@all` becomes a tarpit.
- **Resource Explorer.** This is the right tool for a true account-wide
  search, and neboto already browses it (`@explorer`), but only where an
  aggregator index is configured. It can't be the default backend.

## Consequences

- Don't add fetches to the `@all` path. The stream handlers already refuse to
  cache while `@all` holds foreign rows (see "Search" in `CLAUDE.md`).
- Coverage must stay visible: per-chip match counts, and `–` for a visited
  service with no warm cache.
- A multi-region or multi-service fan-out is its own feature with its own
  design (#92), not an extension of `@all`.
- #123 is the closed issue that searches land on.

# Lazy fetches deliver results via one apply-closure event

Every lazy-section fetch used to have its own typed `Event::*Loaded` variant
(~200 of the Event enum's 224 variants) and its own one-line handler arm.
We replaced them with a single `Event::Lazy(LazyApply)` variant carrying an
epoch-stamped `Box<dyn FnOnce(&mut App) + Send>`: the spawned fetch wraps its
result in a closure that writes into the right `LazyMap`, and the one handler
arm runs it — or drops it when the epoch is stale (an in-flight fetch from a
previous account/region must never land in a fresh `LazyStore`).

## Considered options

- **Apply-closure event (chosen)** — deepest collapse; handlers that do more
  than insert just write a bigger closure; the stale-epoch check lives in one
  place. Cost: the event is opaque in `Debug` output (it prints as
  `Lazy(..)`, not its payload).
- **Slot registry + `Box<dyn Any>`** — events stay data-like and loggable, but
  adds a slot-id namespace, a downcast layer, and per-slot hooks for the
  handlers with side effects.
- **Macro-generated typed events** — keeps the compiler checking each payload,
  but the compiled surface stays 200+ variants and the macro table is another
  parallel list to maintain.

## Consequences

- Don't add new per-fetch `*Loaded` variants for lazy sections; write the
  fetch against `App::trigger_lazy` and let the closure do the apply.
- Debug-logging of events cannot show lazy payloads. If event tracing is ever
  needed, add a label field to `LazyApply` rather than reverting to typed
  variants.

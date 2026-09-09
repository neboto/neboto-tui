//! Generation-tagged load streams (`Event::LoadStream`).
//!
//! The forwarder in `load_resources_async` drops a superseded stream's events
//! at *forwarding* time. That check has a hole: while `handle_event` is
//! blocked in a profile/region/role switch (an awaited client build), the old
//! stream's events pass the forwarder and queue in the channel; by the time
//! they're handled the switch has armed a fresh load, so they append the old
//! credentials' rows to the new list, and a queued `FullyLoaded` caches them
//! and clears `loading` — the new load's batches are then dropped and its
//! `FullyLoaded` re-stamps the stale rows into the cache, so `r` can't fix it.
//! The fix re-checks the generation when the event is handled; this pins it.

use super::*;
use crate::event::LoadProgress;
use std::sync::atomic::Ordering;

fn ec2_mock() -> Box<dyn Resource> {
    all_mocks()
        .into_iter()
        .find(|(svc, _, _)| *svc == ServiceType::EC2)
        .map(|(_, _, r)| r)
        .expect("an EC2 mock in the registry")
}

fn progress() -> LoadProgress {
    LoadProgress {
        loaded_count: 1,
        total_count: None,
        status_message: None,
    }
}

fn tagged(generation: u64, event: Event) -> Event {
    Event::LoadStream {
        generation,
        event: Box::new(event),
    }
}

/// A fresh EC2 load armed (what `install_new_clients` leaves behind), with
/// the stale stream's generation one behind.
fn armed_load(app: &mut App) -> (u64, u64) {
    app.current_service = Some(ServiceType::EC2);
    app.search_active = false;
    app.search_query.clear();
    app.load_service_resources();
    assert!(app.loading);
    let fresh = app.load_generation.load(Ordering::SeqCst);
    (fresh - 1, fresh)
}

#[tokio::test]
async fn stale_generation_batches_are_dropped_at_handling_time() {
    let (mut app, tx, _rx) = test_app().await;
    let (stale, fresh) = armed_load(&mut app);

    // The old credentials' stream, already past the forwarder.
    app.handle_event(
        tagged(
            stale,
            Event::ResourcesPartiallyLoaded {
                service: ServiceType::EC2,
                resources: vec![ec2_mock()],
                progress: progress(),
            },
        ),
        &tx,
    )
    .await
    .unwrap();
    app.handle_event(
        tagged(
            stale,
            Event::ResourcesFullyLoaded {
                service: ServiceType::EC2,
                total_count: 1,
            },
        ),
        &tx,
    )
    .await
    .unwrap();
    assert!(app.resources.is_empty(), "stale batch must not land");
    assert!(app.loading, "stale FullyLoaded must not clear the fresh load");
    assert!(
        app.cache
            .get(&ServiceType::EC2, &app.current_region, None)
            .is_none(),
        "stale FullyLoaded must not stamp the cache"
    );

    // The fresh stream still lands normally.
    app.handle_event(
        tagged(
            fresh,
            Event::ResourcesPartiallyLoaded {
                service: ServiceType::EC2,
                resources: vec![ec2_mock(), ec2_mock()],
                progress: progress(),
            },
        ),
        &tx,
    )
    .await
    .unwrap();
    app.handle_event(
        tagged(
            fresh,
            Event::ResourcesFullyLoaded {
                service: ServiceType::EC2,
                total_count: 2,
            },
        ),
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.resources.len(), 2);
    assert!(!app.loading);
    assert_eq!(
        app.cache
            .get(&ServiceType::EC2, &app.current_region, None)
            .map(|v| v.len()),
        Some(2)
    );
}

#[tokio::test]
async fn stale_load_error_does_not_clear_the_fresh_load() {
    let (mut app, tx, _rx) = test_app().await;
    let (stale, _fresh) = armed_load(&mut app);
    app.handle_event(
        tagged(
            stale,
            Event::ResourceLoadError {
                service: ServiceType::EC2,
                error: "expired token".to_string(),
            },
        ),
        &tx,
    )
    .await
    .unwrap();
    assert!(app.loading);
    assert!(app.error_message.is_none());
}

#[tokio::test]
async fn untagged_load_events_still_take_the_plain_path() {
    let (mut app, tx, _rx) = test_app().await;
    armed_load(&mut app);
    app.handle_event(
        Event::ResourcesPartiallyLoaded {
            service: ServiceType::EC2,
            resources: vec![ec2_mock()],
            progress: progress(),
        },
        &tx,
    )
    .await
    .unwrap();
    assert_eq!(app.resources.len(), 1);
}

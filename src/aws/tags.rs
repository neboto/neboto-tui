//! Surfacing failed list-time tag calls (#28).
//!
//! Services that fill `tags` during the list load fan out one tag call per
//! resource. Mapping a failure to an empty map makes a permission gap or
//! throttle read as "No tags" — indistinguishable from an untagged resource.
//! [`take_tags`] keeps the load best-effort but sends one
//! `ResourceLoadWarning`, so the `Partial load —` line says what happened.

use crate::aws::service::ServiceType;
use crate::event::Event;
use tokio::sync::mpsc;

/// Unwrap one phase's tag results (failures become `T::default()`), sending
/// a single warning for the phase when any call failed. `warned` holds the
/// warning to once per load for loops that fan out phase after phase (a
/// denied call stays denied, so repeating it per batch is noise).
pub fn take_tags<T: Default>(
    results: Vec<Result<T, String>>,
    service: ServiceType,
    what: &str,
    warned: &mut bool,
    event_tx: &mpsc::UnboundedSender<Event>,
) -> Vec<T> {
    let failed = results.iter().filter(|r| r.is_err()).count();
    if !*warned {
        if let Some(Err(e)) = results.iter().find(|r| r.is_err()) {
            *warned = true;
            let _ = event_tx.send(Event::ResourceLoadWarning {
                service,
                warning: format!("{what} tags ({failed} of {} failed): {e}", results.len()),
            });
        }
    }
    results.into_iter().map(|r| r.unwrap_or_default()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    type Tags = HashMap<String, String>;

    #[test]
    fn a_failure_warns_once_and_defaults_the_row() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut warned = false;
        let tagged: Tags = [("team".to_string(), "x".to_string())].into();
        let out = take_tags(
            vec![Ok(tagged.clone()), Err("AccessDenied: no".to_string())],
            ServiceType::Backup,
            "vault",
            &mut warned,
            &tx,
        );
        assert_eq!(out, vec![tagged, Tags::new()]);
        match rx.try_recv() {
            Ok(Event::ResourceLoadWarning { warning, .. }) => {
                assert_eq!(warning, "vault tags (1 of 2 failed): AccessDenied: no")
            }
            _ => panic!("expected one warning"),
        }
        // A second failing phase under the same flag stays quiet.
        take_tags::<Tags>(vec![Err("again".into())], ServiceType::Backup, "plan", &mut warned, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn all_ok_sends_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut warned = false;
        take_tags::<Tags>(vec![Ok(Tags::new())], ServiceType::Backup, "vault", &mut warned, &tx);
        assert!(!warned);
        assert!(rx.try_recv().is_err());
    }
}

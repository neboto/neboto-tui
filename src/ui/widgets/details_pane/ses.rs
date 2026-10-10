use super::*;

// ── SES identity split pane ────────────────────────────────────────────────────

pub(super) fn render_ses_identity_split(app: &App, i: &SesIdentity, area: Rect, frame: &mut Frame) {
    let (status, color) = if i.verified_for_sending {
        ("verified", theme::success())
    } else {
        match i.verification_status.as_deref() {
            Some("SUCCESS") => ("verified", theme::success()),
            Some("PENDING") => ("pending", theme::warning()),
            Some(other) => (other, theme::error()),
            None => ("unverified", theme::error()),
        }
    };
    render_athena_chrome(
        app,
        "SES Identity",
        &i.name,
        i.identity_type.as_deref().unwrap_or(""),
        status,
        color,
        &descriptor_tabs(app, &crate::aws::services::ses::SES_IDENTITY_SECTIONS),
        area,
        frame,
    );
}

pub(super) fn bool_row(key: &str, v: bool) -> (String, String) {
    (
        key.to_string(),
        if v { "✓ Yes".to_string() } else { "✗ No".to_string() },
    )
}

pub fn ses_identity_section_lines(
    i: &SesIdentity,
    section: SesIdentityDetailSection,
    account: Option<&SesAccountInfo>,
) -> Vec<(String, String)> {
    match section {
        SesIdentityDetailSection::Overview => {
            let mut rows = vec![("Identity".to_string(), i.name.clone())];
            if let Some(t) = &i.identity_type {
                rows.push(("Type".to_string(), t.clone()));
            }
            if let Some(v) = &i.verification_status {
                rows.push(("Verification".to_string(), v.clone()));
            }
            rows.push(bool_row("Verified for Sending", i.verified_for_sending));
            rows.push(bool_row("Sending Enabled", i.sending_enabled));
            rows.push(bool_row("Feedback Forwarding", i.feedback_forwarding));
            if let Some(cs) = &i.configuration_set {
                rows.push(("Configuration Set".to_string(), cs.clone()));
            }

            // Account-level posture (fetched once via GetAccount) — the same for
            // every identity, shown here so the numbers are always in view.
            if let Some(a) = account {
                rows.push((String::new(), String::new()));
                rows.push(("Account".to_string(), String::new())); // group header
                rows.push(bool_row("  Sending Enabled", a.sending_enabled));
                rows.push(bool_row("  Production Access", a.production_access));
                if let Some(e) = &a.enforcement_status {
                    rows.push(("  Enforcement".to_string(), e.clone()));
                }
                rows.push((
                    "  Max Send Rate".to_string(),
                    format!("{:.1}/sec", a.max_send_rate),
                ));
                rows.push((
                    "  24h Send Quota".to_string(),
                    if a.max_24h_send < 0.0 {
                        "Unlimited".to_string()
                    } else {
                        format!("{:.0}", a.max_24h_send)
                    },
                ));
                rows.push((
                    "  Sent (last 24h)".to_string(),
                    format!("{:.0}", a.sent_last_24h),
                ));
            }
            rows
        }
        SesIdentityDetailSection::Dkim => {
            let mut rows = vec![bool_row("Signing Enabled", i.dkim_signing_enabled)];
            if let Some(s) = &i.dkim_status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(o) = &i.dkim_origin {
                rows.push(("Origin".to_string(), o.clone()));
            }
            if let Some(k) = &i.dkim_current_key_length {
                rows.push(("Current Key Length".to_string(), k.clone()));
            }
            if let Some(k) = &i.dkim_next_key_length {
                rows.push(("Next Key Length".to_string(), k.clone()));
            }
            if !i.dkim_tokens.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push(("Tokens".to_string(), String::new())); // group header
                for t in &i.dkim_tokens {
                    rows.push((format!(" {}", t), String::new()));
                }
            }
            rows
        }
        SesIdentityDetailSection::MailFrom => {
            if i.mail_from_domain.is_none() {
                return vec![(String::new(), "No custom MAIL FROM domain configured".to_string())];
            }
            let mut rows = Vec::new();
            if let Some(d) = &i.mail_from_domain {
                rows.push(("MAIL FROM Domain".to_string(), d.clone()));
            }
            if let Some(s) = &i.mail_from_status {
                rows.push(("Status".to_string(), s.clone()));
            }
            if let Some(b) = &i.mail_from_behavior {
                rows.push(("On MX Failure".to_string(), b.clone()));
            }
            rows
        }
        SesIdentityDetailSection::Tags => {
            if i.tags.is_empty() {
                return vec![(String::new(), "No tags".to_string())];
            }
            let mut keys: Vec<&String> = i.tags.keys().collect();
            keys.sort();
            let mut rows = vec![("Tags".to_string(), String::new())]; // group header
            for k in keys {
                rows.push((format!("  {}", k), i.tags[k].clone()));
            }
            rows
        }
    }
}

// ── SES configuration-set split pane ────────────────────────────────────────────

pub(super) fn render_ses_config_set_split(app: &App, cs: &SesConfigSet, area: Rect, frame: &mut Frame) {
    let (status, color) = match cs.sending_enabled {
        Some(false) => ("sending paused", theme::error()),
        Some(true) => ("sending enabled", theme::success()),
        None => ("", theme::text_dim()),
    };
    render_athena_chrome(
        app,
        "SES Configuration Set",
        &cs.name,
        "configuration set",
        status,
        color,
        &descriptor_tabs(app, &crate::aws::services::ses::SES_CONFIG_SET_SECTIONS),
        area,
        frame,
    );
}

pub fn ses_config_set_section_lines(
    cs: &SesConfigSet,
    section: SesConfigSetDetailSection,
    dests: Option<&crate::lazy::Lazy<Vec<crate::aws::services::ses::SesEventDest>>>,
) -> Vec<(String, String)> {
    match section {
        SesConfigSetDetailSection::Overview => {
            let mut rows = vec![("Name".to_string(), cs.name.clone())];
            if !cs.detail_ok {
                rows.append(&mut error_rows(
                    "GetConfigurationSet failed — showing name only",
                ));
                return rows;
            }
            if let Some(s) = cs.sending_enabled {
                rows.push(bool_row("Sending Enabled", s));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Reputation".to_string(), String::new())); // group header
            if let Some(r) = cs.reputation_enabled {
                rows.push(bool_row("  Reputation Metrics", r));
            }
            if let Some(f) = &cs.last_fresh_start {
                rows.push(("  Last Fresh Start".to_string(), f.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Delivery".to_string(), String::new())); // group header
            rows.push((
                "  TLS Policy".to_string(),
                cs.tls_policy.clone().unwrap_or_else(|| "OPTIONAL (default)".to_string()),
            ));
            if let Some(p) = &cs.sending_pool {
                rows.push(("  Dedicated IP Pool".to_string(), p.clone()));
            }
            if let Some(s) = cs.max_delivery_secs {
                rows.push(("  Max Delivery Time".to_string(), format!("{} s", s)));
            }
            if cs.redirect_domain.is_some() || cs.https_policy.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("Open/Click Tracking".to_string(), String::new()));
                if let Some(d) = &cs.redirect_domain {
                    rows.push(("  Redirect Domain".to_string(), d.clone()));
                }
                if let Some(h) = &cs.https_policy {
                    rows.push(("  HTTPS Policy".to_string(), h.clone()));
                }
            }
            if !cs.suppressed_reasons.is_empty() {
                rows.push((String::new(), String::new()));
                rows.push((
                    "Suppression Overrides".to_string(),
                    cs.suppressed_reasons.join(", "),
                ));
            }
            if cs.vdm_engagement.is_some() || cs.vdm_guardian.is_some() {
                rows.push((String::new(), String::new()));
                rows.push(("Virtual Deliverability Manager".to_string(), String::new()));
                if let Some(e) = &cs.vdm_engagement {
                    rows.push(("  Engagement Metrics".to_string(), e.clone()));
                }
                if let Some(g) = &cs.vdm_guardian {
                    rows.push(("  Optimized Shared Delivery".to_string(), g.clone()));
                }
            }
            if let Some(a) = &cs.archiving_arn {
                rows.push((String::new(), String::new()));
                rows.push(("Mail Manager Archive".to_string(), a.clone()));
            }
            rows
        }
        SesConfigSetDetailSection::EventDestinations => match dests {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading event destinations…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(ds)) => {
                if ds.is_empty() {
                    return vec![(
                        "".to_string(),
                        "No event destinations — send/bounce/complaint events are not published"
                            .to_string(),
                    )];
                }
                let mut rows = vec![(format!("Event Destinations ({})", ds.len()), String::new())];
                rows.push((String::new(), String::new()));
                for d in ds {
                    rows.push((format!("{} · {}", d.kind, d.name), String::new()));
                    rows.push((
                        "  Enabled".to_string(),
                        if d.enabled {
                            "✓ Yes".to_string()
                        } else {
                            "✗ No".to_string()
                        },
                    ));
                    rows.push(("  Event Types".to_string(), d.event_types.join(", ")));
                    if let Some(t) = &d.target {
                        rows.push(("  Target".to_string(), t.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
                rows
            }
        },
        SesConfigSetDetailSection::Tags => tag_rows(&cs.tags),
    }
}

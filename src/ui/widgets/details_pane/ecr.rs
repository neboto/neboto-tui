use super::*;

// ── ECR repository split pane ────────────────────────────────────────────────

pub(super) fn render_ecr_repo_split(app: &App, repo: &EcrRepository, area: Rect, frame: &mut Frame) {
    
    let focused = app.details_focused;
    let footer = detail_footer(app, focused, 4, "");
    let mut block = theme::pane_block("ECR Repository", focused);
    block = block.title_bottom(
        Line::from(Span::styled(footer, Style::default().fg(theme::text_dim())))
            .right_aligned(),
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let header = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                repo.name.clone(),
                Style::default().fg(crate::ui::theme::text_primary()).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(repo.uri.clone(), Style::default().fg(theme::text_dim())),
        ]),
        Line::raw(""),
    ];
    let header_h = header.len() as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);
    frame.render_widget(Paragraph::new(header), chunks[0]);
    render_hr(chunks[1], frame);
    render_section_tab_bar(app, 
        chunks[2],
        frame,
        &descriptor_tabs(app, &crate::aws::services::ecr::ECR_REPO_SECTIONS),
    );
    render_hr(chunks[3], frame);
    render_split_section_body(app, chunks[4], frame);
}

pub fn ecr_repo_section_lines(
    repo: &EcrRepository,
    section: crate::aws::services::ecr::EcrRepoDetailSection,
    images: &[&crate::aws::services::ecr::EcrImage],
    list_loading: bool,
    lifecycle: Option<&crate::lazy::Lazy<String>>,
    drift: &[crate::aws::services::ecr::DigestDrift],
    running: &[crate::aws::services::ecr::DigestUse],
) -> Vec<(String, String)> {
    let running_users: usize = running.iter().map(|u| u.tasks).sum();
    use crate::aws::services::ecr::DigestStatus;
    match section {
        EcrRepoDetailSection::Details => {
            // Mutable tags are only a live risk once something runs a digest
            // from here: re-pushing a tag then strands that digest untagged.
            let mutability = if repo.tags_mutable() && running_users > 0 {
                format!(
                    "⚠ {} — {running_users} running container{} pull from here; re-pushing a tag leaves their digest untagged",
                    repo.image_tag_mutability,
                    if running_users == 1 { "" } else { "s" },
                )
            } else {
                repo.image_tag_mutability.clone()
            };
            let mut rows = drift_rows(drift);
            rows.extend([
                ("Repository".to_string(), repo.name.clone()),
                ("URI".to_string(), repo.uri.clone()),
                ("Tag Mutability".to_string(), mutability),
                ("Scan on Push".to_string(), if repo.scan_on_push { "✓ enabled" } else { "✗ disabled" }.to_string()),
                ("Encryption".to_string(), repo.encryption_type.clone()),
            ]);
            if let Some(k) = &repo.kms_key {
                rows.push(("KMS Key".to_string(), k.clone()));
            }
            if let Some(c) = &repo.created {
                rows.push(("Created".to_string(), c.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("ARN".to_string(), repo.arn.clone()));
            rows
        }
        EcrRepoDetailSection::Images => {
            // The Images tab's rows for this repo (the list load fetched
            // them) — filtered, not fetched again.
            if images.is_empty() {
                return if list_loading {
                    vec![("".to_string(), "Loading…".to_string())]
                } else {
                    vec![(" No images in this repository".to_string(), String::new())]
                };
            }
            let seen = images[0].repo_images_seen;
            let complete = images[0].repo_images_complete;
            let heading = if seen > images.len() || !complete {
                format!(
                    "Images (newest {} of {}{})",
                    images.len(),
                    seen,
                    if complete { "" } else { "+" }
                )
            } else {
                format!("Images ({}, newest first)", images.len())
            };
            let mut rows = vec![(heading, String::new())];
            rows.push((String::new(), String::new()));
            for img in images {
                // Group header: the image tag(s), or <untagged>.
                let tag_display = if img.tags.is_empty() {
                    "<untagged>".to_string()
                } else {
                    img.tags.join(", ")
                };
                rows.push((tag_display, String::new()));

                rows.push(("  URI".to_string(), img.image_ref()));
                rows.push(("  Digest".to_string(), img.digest.clone()));
                if drift.iter().any(|d| d.digest == img.digest && d.status == DigestStatus::Untagged) {
                    rows.push((String::new(), "⚠ untagged but running — an untagged-expiry rule can delete it".to_string()));
                }
                if let Some(p) = &img.pushed_at {
                    rows.push(("  Pushed".to_string(), p.clone()));
                }
                rows.push((
                    "  Last Pulled".to_string(),
                    img.last_pulled_at.clone().unwrap_or_else(|| "Never".to_string()),
                ));
                rows.push(("  Size".to_string(), img.size_display()));
                if let Some(a) = &img.artifact_media_type {
                    rows.push(("  Artifact Type".to_string(), a.clone()));
                }
                if let Some(m) = &img.manifest_media_type {
                    rows.push(("  Manifest Type".to_string(), m.clone()));
                }

                // Scan status + findings.
                let scan = if img.scan_status.is_empty() {
                    "Not scanned".to_string()
                } else {
                    img.scan_status.clone()
                };
                rows.push(("  Scan Status".to_string(), scan));
                let breakdown = img.vuln_breakdown();
                if !breakdown.is_empty() {
                    rows.push(("  Vulnerabilities".to_string(), breakdown));
                } else if img.scan_status == "COMPLETE" {
                    rows.push(("  Vulnerabilities".to_string(), "✓ No findings".to_string()));
                } else if let Some(d) = &img.scan_status_description {
                    if !d.is_empty() {
                        rows.push(("  Scan Detail".to_string(), d.clone()));
                    }
                }

                rows.push((String::new(), String::new()));
            }
            rows
        }
        EcrRepoDetailSection::LifecyclePolicy => match lifecycle {
            None | Some(crate::lazy::Lazy::Loading) => {
                vec![("".to_string(), "Loading lifecycle policy…".to_string())]
            }
            Some(crate::lazy::Lazy::Error(e)) => error_rows(e),
            Some(crate::lazy::Lazy::Loaded(text)) => {
                if text.is_empty() {
                    return vec![(" No lifecycle policy configured".to_string(), String::new())];
                }
                let mut rows = lifecycle_rows(repo, text, images, running, chrono::Utc::now().timestamp());
                // The policy itself, below what it means.
                let pretty = serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_else(|| text.clone());
                rows.push(("Lifecycle Policy".to_string(), String::new()));
                rows.push((String::new(), String::new()));
                for line in pretty.lines() {
                    rows.push((format!("  {}", line), String::new()));
                }
                rows
            }
        },
        EcrRepoDetailSection::Tags => tag_rows(&repo.tags),
    }
}

/// Body rows for an ECR image's split pane. `users` is `Some((rows,
/// ecs_cache_warm))` for the Used By section (computed by the caller from
/// in-memory ECS data), `None` otherwise.
pub fn ecr_image_section_lines(
    img: &crate::aws::services::ecr::EcrImage,
    section: crate::aws::services::ecr::EcrImageDetailSection,
    findings: Option<&crate::lazy::Lazy<crate::aws::services::ecr::EcrScanFindings>>,
    users: Option<(&[crate::aws::services::ecr::EcrImageUser], bool)>,
    drift: &[crate::aws::services::ecr::DigestDrift],
) -> Vec<(String, String)> {
    use crate::aws::services::ecr::EcrImageDetailSection as S;
    use crate::lazy::Lazy;
    match section {
        S::Overview => {
            let mut rows = drift_rows(drift);
            rows.extend([
                ("Repository".to_string(), img.repo_name.clone()),
                (
                    "Tags".to_string(),
                    if img.tags.is_empty() { "<untagged>".to_string() } else { img.tags.join(", ") },
                ),
                ("Digest".to_string(), img.digest.clone()),
                ("URI".to_string(), img.image_ref()),
                ("Size".to_string(), img.size_display()),
            ]);
            if let Some(p) = &img.pushed_at {
                rows.push(("Pushed".to_string(), p.clone()));
            }
            rows.push((
                "Last Pulled".to_string(),
                img.last_pulled_at.clone().unwrap_or_else(|| "Never".to_string()),
            ));
            if let Some(a) = &img.artifact_media_type {
                rows.push(("Artifact Type".to_string(), a.clone()));
            }
            if let Some(m) = &img.manifest_media_type {
                rows.push(("Manifest Type".to_string(), m.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push(("Scan".to_string(), String::new()));
            rows.push((
                "  Status".to_string(),
                if img.scan_status.is_empty() { "Not scanned".to_string() } else { img.scan_status.clone() },
            ));
            let breakdown = img.vuln_breakdown();
            if !breakdown.is_empty() {
                rows.push(("  Vulnerabilities".to_string(), breakdown));
            } else if img.scan_status == "COMPLETE" {
                rows.push(("  Vulnerabilities".to_string(), "✓ No findings".to_string()));
            }
            if let Some(d) = img.scan_status_description.as_ref().filter(|d| !d.is_empty()) {
                rows.push(("  Detail".to_string(), d.clone()));
            }
            rows.push((String::new(), String::new()));
            rows.push((
                " · Last Pulled is ECR's own record (refreshed at most daily)".to_string(),
                String::new(),
            ));
            rows
        }
        S::Findings => match findings {
            None | Some(Lazy::Loading) => vec![("".to_string(), "Loading…".to_string())],
            Some(Lazy::Error(e)) => error_rows(e),
            Some(Lazy::Loaded(f)) => ecr_findings_rows(f),
        },
        S::UsedBy => {
            let (list, warm) = users.unwrap_or((&[][..], false));
            let mut rows = drift_rows(drift);
            if list.is_empty() {
                rows.push((" No ECS task or task definition found using this image".to_string(), String::new()));
            } else {
                rows.push((format!("ECS ({})", list.len()), String::new()));
                rows.push((String::new(), String::new()));
                for u in list {
                    rows.push((u.name.clone(), String::new()));
                    rows.push((format!("  {}", u.kind), u.id.clone()));
                    rows.push(("  Container".to_string(), u.container.clone()));
                    if !u.status.is_empty() {
                        rows.push(("  Status".to_string(), u.status.clone()));
                    }
                    rows.push((String::new(), String::new()));
                }
            }
            rows.push((String::new(), String::new()));
            // Coverage, so an empty list never reads as "nothing uses it".
            if warm {
                rows.push((" · searched the loaded ECS tasks (running + recently stopped)".to_string(), String::new()));
            } else {
                rows.push((" · ECS not loaded — open @ecs, then come back to search its tasks".to_string(), String::new()));
            }
            rows.push((" · task definitions count once their details have been opened".to_string(), String::new()));
            rows
        }
    }
}

/// `5h` / `6d` — a compact age or wait.
fn span(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 86_400 { format!("{}h", secs / 3_600) } else { format!("{}d", secs / 86_400) }
}

/// What the lifecycle policy does to the loaded images (#157), above the raw
/// JSON: warnings first (in-use images it will act on, and the
/// mutable-tags + untagged-expiry + running-digest combination), then one
/// block per rule. Zero API — evaluated from the Images rows and the warm
/// ECS tasks; `now` is a parameter so tests can pin the clock.
pub(super) fn lifecycle_rows(
    repo: &EcrRepository,
    text: &str,
    images: &[&crate::aws::services::ecr::EcrImage],
    running: &[crate::aws::services::ecr::DigestUse],
    now: i64,
) -> Vec<(String, String)> {
    use crate::aws::services::ecr::{evaluate_lifecycle, parse_lifecycle, LIFECYCLE_DUE_DAYS};
    let rules = match parse_lifecycle(text) {
        Ok(r) => r,
        Err(e) => return vec![(String::new(), format!("⚠ couldn't read the policy ({e}) — raw JSON below")), (String::new(), String::new())],
    };
    let outcomes = evaluate_lifecycle(&rules, images, now);
    let users = |digest: &str| -> Vec<&crate::aws::services::ecr::DigestUse> {
        running.iter().filter(|u| u.digest == digest).collect()
    };
    let when = |days_left: i64| if days_left <= 0 { "now".to_string() } else { format!("in {days_left}d") };
    let mut rows: Vec<(String, String)> = Vec::new();

    // The incident pattern: mutable tags + an untagged age rule + a consumer
    // running a digest. Ages count from *push*, so a digest a re-push
    // untags may already be past the limit — gone at the next run.
    if repo.tags_mutable() && !running.is_empty() {
        if let Some(r) = rules.iter().find(|r| {
            r.action == "expire" && r.tag_status == "untagged" && r.count_type == "sinceImagePushed"
        }) {
            rows.push((
                String::new(),
                format!(
                    "⚠ Tags are {} and priority {} expires untagged images {} days after push, while running tasks pin digests here: \
                     re-pushing a tag they use leaves their digest untagged, and if it was pushed over {} days ago the next lifecycle run (within 24h) deletes it",
                    repo.image_tag_mutability, r.priority, r.count_number, r.count_number
                ),
            ));
        }
    }
    for o in &outcomes {
        for f in o.eligible.iter().chain(o.due.iter()) {
            for u in users(&f.image.digest) {
                rows.push((
                    String::new(),
                    format!(
                        "⚠ {} — {} under priority {} {}, in use by {}",
                        f.image.label,
                        o.rule.verb_past(),
                        o.rule.priority,
                        when(f.days_left),
                        u.who()
                    ),
                ));
            }
        }
        if let Some(img) = o.next_push_expires {
            for u in users(&img.digest) {
                rows.push((
                    String::new(),
                    format!("⚠ {} — the next matching push {}s it (priority {}), in use by {}", img.label, o.rule.verb(), o.rule.priority, u.who()),
                ));
            }
        }
    }
    if !rows.is_empty() {
        rows.push((String::new(), String::new()));
    }

    let partial = images
        .first()
        .map(|i| i.repo_images_seen > images.len() || !i.repo_images_complete)
        .unwrap_or(false);
    rows.push((format!("Rules ({})", rules.len()), String::new()));
    rows.push((String::new(), String::new()));
    for o in &outcomes {
        let r = &o.rule;
        rows.push((format!("Priority {}", r.priority), r.summary()));
        if let Some(d) = r.description.as_ref().filter(|d| !d.is_empty()) {
            rows.push(("  Description".to_string(), d.clone()));
        }
        if !r.evaluable() {
            rows.push((String::new(), "· not evaluated — an unknown count type".to_string()));
            rows.push((String::new(), String::new()));
            continue;
        }
        rows.push(("  Matches".to_string(), format!("{} loaded image{}", o.matched, if o.matched == 1 { "" } else { "s" })));
        if !o.eligible.is_empty() {
            rows.push((
                "  Eligible Now".to_string(),
                format!("{} — ECR acts within 24h", o.eligible.len()),
            ));
            for f in &o.eligible {
                rows.push((image_fate_line(f.image, now, None, &users(&f.image.digest)), String::new()));
            }
        }
        if !o.due.is_empty() {
            rows.push((format!("  Due in {LIFECYCLE_DUE_DAYS} Days"), o.due.len().to_string()));
            for f in &o.due {
                let line = image_fate_line(f.image, now, Some(when(f.days_left)), &users(&f.image.digest));
                rows.push((line, String::new()));
            }
        }
        if let Some(img) = o.next_push_expires {
            rows.push(("  Next Push".to_string(), format!("{}s {} (the oldest of {} kept)", r.verb(), img.label, r.count_number)));
        }
        if r.count_type == "imageCountMoreThan" && partial && o.eligible.is_empty() {
            rows.push((String::new(), format!("· list cut to the newest {} — can't tell what this rule keeps", images.len())));
        } else if o.eligible.is_empty() && o.due.is_empty() && o.next_push_expires.is_none() {
            rows.push((String::new(), format!("✓ nothing to {} in the next {LIFECYCLE_DUE_DAYS} days", r.verb())));
        }
        rows.push((String::new(), String::new()));
    }
    if partial {
        rows.push((
            String::new(),
            format!(
                "· partial: evaluated over the newest {} of {} images — older images beyond the cut may match too",
                images.len(),
                images.first().map(|i| i.repo_images_seen).unwrap_or_default()
            ),
        ));
    }
    rows.push((
        String::new(),
        "· a preview from the loaded images; ECR's own run can differ (manifest lists, referrers) — the console's lifecycle preview is authoritative".to_string(),
    ));
    rows.push((String::new(), String::new()));
    rows
}

/// `    orders-worker@d6a4f2b5eaa7 · pushed 6d ago · in 1d`, ⚠-prefixed and
/// `· in use` when running tasks use it.
fn image_fate_line(
    img: &crate::aws::services::ecr::EcrImage,
    now: i64,
    when: Option<String>,
    users: &[&crate::aws::services::ecr::DigestUse],
) -> String {
    let age = img.pushed_secs.map(|p| format!(" · pushed {} ago", span(now - p))).unwrap_or_default();
    let when = when.map(|w| format!(" · {w}")).unwrap_or_default();
    if users.is_empty() {
        format!("    {}{age}{when}", img.label)
    } else {
        format!("    ⚠ {}{age}{when} · in use", img.label)
    }
}

/// Digest-drift warnings (#156) as a block that leads a section, followed
/// by a spacer; nothing when there's no drift.
pub(super) fn drift_rows(drift: &[crate::aws::services::ecr::DigestDrift]) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = drift.iter().map(|d| (String::new(), d.warning())).collect();
    if !rows.is_empty() {
        rows.push((String::new(), String::new()));
    }
    rows
}

/// The Findings section body: a summary, then one group per finding,
/// worst first (already sorted by the fetch).
pub(super) fn ecr_findings_rows(f: &crate::aws::services::ecr::EcrScanFindings) -> Vec<(String, String)> {
    let mut rows = vec![(
        "Scan Status".to_string(),
        if f.scan_status.is_empty() { "—".to_string() } else { f.scan_status.clone() },
    )];
    if !f.scanner.is_empty() {
        rows.push(("Scanner".to_string(), f.scanner.clone()));
    }
    if let Some(d) = f.status_description.as_ref().filter(|d| !d.is_empty()) {
        rows.push(("Detail".to_string(), d.clone()));
    }
    if let Some(c) = &f.completed_at {
        rows.push(("Completed".to_string(), c.clone()));
    }
    if let Some(u) = &f.vuln_db_updated_at {
        rows.push(("Vuln DB Updated".to_string(), u.clone()));
    }
    rows.push((String::new(), String::new()));
    if f.findings.is_empty() {
        let note = if f.scan_status == "COMPLETE" {
            "✓ No findings"
        } else {
            "No findings to show"
        };
        rows.push((String::new(), note.to_string()));
        return rows;
    }
    let shown = f.findings.len();
    rows.push((
        if f.truncated > 0 {
            format!("Findings ({} worst of {})", shown, shown + f.truncated)
        } else {
            format!("Findings ({})", shown)
        },
        String::new(),
    ));
    rows.push((String::new(), String::new()));
    for x in &f.findings {
        let sev = if x.severity.is_empty() { "UNDEFINED" } else { x.severity.as_str() };
        rows.push((format!("{} {}", sev, x.id), String::new()));
        if let Some(p) = &x.package {
            let ver = x.installed.as_deref().map(|v| format!(" {}", v)).unwrap_or_default();
            rows.push(("  Package".to_string(), format!("{}{}", p, ver)));
        }
        if let Some(v) = &x.fixed_in {
            rows.push(("  Fixed In".to_string(), v.clone()));
        } else if let Some(fa) = &x.fix_available {
            rows.push(("  Fix Available".to_string(), fa.clone()));
        }
        if let Some(sc) = x.score {
            rows.push(("  Score".to_string(), format!("{:.1}", sc)));
        }
        if let Some(st) = &x.status {
            rows.push(("  Status".to_string(), st.clone()));
        }
        if let Some(e) = &x.exploit_available {
            rows.push(("  Exploit Available".to_string(), e.clone()));
        }
        if let Some(u) = &x.url {
            rows.push(("  URL".to_string(), u.clone()));
        }
        if let Some(d) = x.description.as_ref().filter(|d| !d.is_empty()) {
            let one_line = d.split_whitespace().collect::<Vec<_>>().join(" ");
            let short: String = one_line.chars().take(200).collect();
            let ell = if one_line.chars().count() > 200 { "…" } else { "" };
            rows.push(("  Description".to_string(), format!("{}{}", short, ell)));
        }
        rows.push((String::new(), String::new()));
    }
    rows
}

#[cfg(test)]
mod lifecycle_rows_tests {
    use super::*;
    use crate::aws::services::ecr::DigestUse;

    const NOW: i64 = 1_800_000_000;
    const HOST: &str = "123456789012.dkr.ecr.us-east-1.amazonaws.com";
    const POLICY: &str = r#"{"rules":[{"rulePriority":1,"description":"expire untagged after 7 days","selection":{"tagStatus":"untagged","countType":"sinceImagePushed","countUnit":"days","countNumber":7},"action":{"type":"expire"}}]}"#;

    fn image(n: u32, tags: &[&str], age_days: i64) -> crate::aws::services::ecr::EcrImage {
        let mut b = aws_sdk_ecr::types::ImageDetail::builder()
            .repository_name("web")
            .image_digest(format!("sha256:{n:064}"))
            .image_pushed_at(aws_smithy_types::DateTime::from_secs(NOW - age_days * 86_400));
        for t in tags {
            b = b.image_tags(*t);
        }
        crate::aws::services::ecr::EcrImage::from_sdk(&b.build(), "web", &format!("{HOST}/web"))
    }

    fn repo(mutability: &str) -> EcrRepository {
        let r = aws_sdk_ecr::types::Repository::builder()
            .repository_name("web")
            .repository_uri(format!("{HOST}/web"))
            .image_tag_mutability(aws_sdk_ecr::types::ImageTagMutability::from(mutability))
            .build();
        // `from_sdk` is private to the service module; the list load is its
        // only caller, so build through the same path the harness mocks use.
        crate::aws::services::ecr::EcrRepository::from_sdk_for_test(&r)
    }

    fn text(rows: &[(String, String)]) -> String {
        rows.iter().map(|(k, v)| format!("{k}|{v}\n")).collect()
    }

    #[test]
    fn the_incident_pattern_is_called_out_with_the_image_at_risk() {
        let stale = image(1, &[], 8);
        let current = image(2, &["latest"], 0);
        let running = [DigestUse { owner: "api".into(), container: "app".into(), digest: stale.digest.clone(), tasks: 2 }];
        let rows = lifecycle_rows(&repo("MUTABLE"), POLICY, &[&current, &stale], &running, NOW);
        let t = text(&rows);
        assert!(t.contains("⚠ Tags are MUTABLE and priority 1 expires untagged images 7 days after push"), "{t}");
        assert!(t.contains("expires under priority 1 now, in use by api/app (2 tasks)"), "{t}");
        assert!(t.contains("Priority 1|expire untagged images 7 days after push"), "{t}");
        assert!(t.contains("  Eligible Now|1 — ECR acts within 24h"), "{t}");
        assert!(t.contains("⚠ web@"), "the image line is flagged as in use: {t}");
    }

    #[test]
    fn quiet_when_nothing_is_at_risk() {
        let fresh = image(1, &[], 1);
        let rows = lifecycle_rows(&repo("IMMUTABLE"), POLICY, &[&fresh], &[], NOW);
        let t = text(&rows);
        assert!(!t.contains("⚠"), "{t}");
        assert!(t.contains("  Due in 7 Days|1"), "{t}");
    }

    #[test]
    fn a_cut_list_says_it_is_partial() {
        let mut newest = image(1, &["v9"], 1);
        newest.repo_images_seen = 250;
        let policy = r#"{"rules":[{"rulePriority":1,"selection":{"tagStatus":"tagged","tagPrefixList":["v"],"countType":"imageCountMoreThan","countNumber":10},"action":{"type":"expire"}}]}"#;
        let t = text(&lifecycle_rows(&repo("MUTABLE"), policy, &[&newest], &[], NOW));
        assert!(t.contains("can't tell what this rule keeps"), "{t}");
        assert!(t.contains("· partial: evaluated over the newest 1 of 250 images"), "{t}");
        assert!(!t.contains("✓ nothing to expire"), "a cut list must not promise nothing expires: {t}");
    }
}

mod app;
mod aws;
mod bookmarks;
mod cli;
mod config;
mod editor;
mod error;
mod export;
mod event;
mod html;
mod lazy;
mod macros;
mod navigation;
mod ownership;
mod references;
mod timeline;
mod search;
mod sections;
mod terraform;
mod tui;
mod ui;

use app::App;
use aws::service::ServiceType;
use error::Result;
use event::{handle_terminal_events, handle_tick_events, EventHandler};
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::time::Duration;
use tui::Tui;
use ui::layout::AppLayout;
use ui::theme;
use ui::widgets::{
    agentcore_tabs, apigw_tabs, backup_tabs, banner, bedrock_tabs, cfn_tabs, cloudfront_tabs, cognito_tabs, config_tabs, controltower_tabs, servicecatalog_tabs, ssm_tabs, cost_tabs, ct_filter_modal, ct_tabs, cw_tabs,
    details_pane,
    dx_tabs,
    ec2_tabs,
    athena_tabs, ecs_tabs, elb_tabs, eventbridge_tabs, fms_tabs, fsx_tabs, gd_tabs, glue_tabs, help_overlay, iam_tabs, idc_tabs, insp_tabs, jump_list, kinesis_tabs, macro_picker, messaging_tabs, s3tables_tabs, sfn_tabs,
    message_log,
    network_firewall_tabs,
    ram_tabs,
    ta_tabs,
    code_tabs,
    organizations_tabs, org_role_selector, profile_selector, quota_service_selector, r53_tabs,
    rds_tabs,
    redshift_tabs,
    region_selector, resolver_tabs, resource_list,
    s3_tabs, search_bar, ses_tabs, service_selector, service_tabs, sh_tabs, splash, sq_tabs,
    ssm_session_modal, tgw_tabs, vpc_tabs,
    waf_tabs,
};

fn main() -> Result<()> {
    // Parse CLI args before the runtime so `--help`/`--version` exit immediately
    // (clap prints and exits on those / on a parse error).
    let cli = <cli::Cli as clap::Parser>::parse();

    // Build the runtime explicitly so we can enlarge the worker-thread stack.
    // `aws-sdk-securityhub`'s `AwsSecurityFinding` / `ResourceDetails` type is
    // enormous (a field for nearly every AWS resource type), so the SDK's
    // generated deserializer builds stack frames that overflow tokio's default
    // ~2 MB worker stack ("thread tokio-runtime-worker has overflowed its
    // stack"). 16 MB is ample headroom and stacks are lazily committed.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .expect("failed to build tokio runtime")
        .block_on(run(cli))
}

async fn run(cli: cli::Cli) -> Result<()> {
    // Initialize app
    let mut app = App::new(cli).await?;

    // Initialize TUI (use Option to allow drop/recreate)
    let mut tui = Some(Tui::new()?);

    // Setup event handler
    let mut event_handler = EventHandler::new();
    let mut event_tx = event_handler.sender();

    // Spawn terminal event handler
    let term_event_tx = event_tx.clone();
    tokio::spawn(async move {
        handle_terminal_events(term_event_tx).await;
    });

    // Spawn tick event handler
    let tick_event_tx = event_tx.clone();
    tokio::spawn(async move {
        handle_tick_events(tick_event_tx, Duration::from_millis(250)).await;
    });

    // Trigger initial resource load — but only if a startup service is set.
    // With no default_service the welcome splash shows and nothing loads until
    // the user picks a service.
    // A `--macro` that opens by switching service supersedes this: the default
    // service's list would be fetched and discarded a moment later.
    if app.current_service.is_some() && !app.macro_supersedes_startup_load() {
        app.load_service_resources();
    }

    // Load AWS account identity in background
    app.spawn_account_info_fetch(&event_tx);

    // Main event loop
    while app.running {
        // Check if we need to load resources
        if app.should_load_resources() {
            app.load_resources_async(&event_tx);
        }

        // Check if an editor or an inline SSM session was requested — both
        // need the TUI fully torn down so the child process owns the TTY.
        if app.editor_requested || app.session_requested {
            let run_session = app.session_requested;
            app.editor_requested = false;
            app.session_requested = false;

            // Save state before suspending
            let had_resources = !app.resources.is_empty();
            let saved_selected_resource_id = app.get_selected_resource_id();

            if let Some(mut current_tui) = tui.take() {
                // 1. Restore terminal fully
                if let Err(e) = current_tui.restore() {
                    app.error_message = Some(format!("Failed to restore terminal: {}", e));
                    tui = Some(current_tui);
                } else {
                    // 2. Drop event handler to stop all background tasks
                    drop(event_handler);
                    tokio::time::sleep(Duration::from_millis(100)).await;

                    // 3. Drop TUI
                    drop(current_tui);

                    // 4. Run the requested action (terminal is now in normal mode)
                    if run_session {
                        app.run_pending_session();
                    } else {
                        let _ = app.open_in_editor();
                    }

                    // 5. Clear stdin aggressively (non-blocking)
                    use crossterm::event::{poll, read};
                    for _ in 0..10 {
                        while poll(Duration::from_millis(0)).unwrap_or(false) {
                            let _ = read();
                        }
                        // Use tokio sleep to not block runtime
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }

                    // 6. Recreate TUI first
                    tui = match Tui::new() {
                        Ok(mut t) => {
                            // Clear terminal and hide cursor
                            let _ = t.terminal().clear();
                            let _ = t.terminal().hide_cursor();
                            Some(t)
                        }
                        Err(_) => break,
                    };

                    // 7. Reset app state (EXPLICITLY preserve banner_visible!)
                    let banner_state = app.banner_visible;
                    app.loading = false;
                    app.loading_started = false;
                    app.loading_progress = None;
                    app.loading_complete = false;
                    // The event channel was torn down mid-stream — any watch
                    // refresh in flight will never complete, so drop its staging.
                    app.watch_staging = None;
                    // search_query intentionally preserved (not cleared) so the
                    // same filtered view + selection can be restored below.
                    app.search_active = false;
                    app.resources.clear();
                    app.filtered_resources.clear();
                    app.selected_index = None;
                    app.resource_list_state.borrow_mut().select(None);
                    app.details_selected_index = None;
                    app.banner_visible = banner_state; // Explicitly restore banner state

                    // 8. Recreate event handler with NEW channel (no old events possible)
                    event_handler = EventHandler::new();
                    event_tx = event_handler.sender();

                    // 9. One more stdin clear before spawning handlers
                    while poll(Duration::from_millis(0)).unwrap_or(false) {
                        let _ = read();
                    }

                    // 10. Spawn fresh event handlers
                    let term_event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        handle_terminal_events(term_event_tx).await;
                    });

                    let tick_event_tx = event_tx.clone();
                    tokio::spawn(async move {
                        handle_tick_events(tick_event_tx, Duration::from_millis(250)).await;
                    });

                    // 11. Reload resources from cache (if they exist)
                    if had_resources {
                        app.load_service_resources();

                        // Only update if resources actually loaded from cache
                        // (cache hit). If cache miss, main loop will handle loading.
                        if !app.resources.is_empty() {
                            app.update_search();
                            if let Some(id) = &saved_selected_resource_id {
                                app.restore_selection_by_id(id);
                            }
                        }
                    }
                }
            }
        }

        // Flat detail view: rebuild the concatenated all-section body and
        // keep the section enum synced to the cursor before drawing.
        app.flat_detail_tick(&event_tx);

        // Finish a deep export (`X` over a list selection) as soon as the lazy
        // detail sections it fired have landed — no second keypress needed.
        app.deep_export_tick(&event_tx);

        // Macro recording (classify the key just dispatched) and playback
        // (inject the next step once the app is quiescent). Runs before the
        // draw so the recorder's step count and the picker are current.
        app.macro_tick(&event_tx);

        // Draw UI first to show input immediately
        if let Some(ref mut t) = tui {
            t.terminal().draw(|frame| render_app(&app, frame))?;
        }

        // Process any pending search updates after drawing input
        // This allows the input character to be visible before results update
        app.process_pending_search();

        // Record any new status messages into the reviewable history (`M`)
        // before the expiry pass below can clear them.
        app.record_message_history();

        // Clear success messages after timeout so help text returns
        app.clear_expired_success_message();

        // Handle events
        if let Some(event) = event_handler.next().await {
            app.handle_event(event, &event_tx).await?;
        }
    }

    // Restore terminal
    if let Some(mut t) = tui {
        t.restore()?;
    }

    Ok(())
}

/// Render one full frame — everything the main loop draws, factored out of
/// the draw closure so the layer-2 harness tests can render to a
/// `TestBackend` without a real terminal.
fn render_app(app: &App, frame: &mut ratatui::Frame) {
    let show_sub_tabs = matches!(
        app.current_service,
        Some(ServiceType::EC2)
            | Some(ServiceType::ECS)
            | Some(ServiceType::IdentityCenter)
            | Some(ServiceType::VPC)
            | Some(ServiceType::S3)
            | Some(ServiceType::CloudFormation)
            | Some(ServiceType::Route53)
            | Some(ServiceType::CloudWatch)
            | Some(ServiceType::RDS)
            | Some(ServiceType::IAM)
            | Some(ServiceType::Organizations)
            | Some(ServiceType::Elb)
            | Some(ServiceType::Messaging)
            | Some(ServiceType::Ssm)
            | Some(ServiceType::Cost)
            | Some(ServiceType::Config)
            | Some(ServiceType::Waf)
            | Some(ServiceType::TransitGateway)
            | Some(ServiceType::Route53Resolver)
            | Some(ServiceType::DirectConnect)
            | Some(ServiceType::ApiGateway)
            | Some(ServiceType::ServiceQuotas)
            | Some(ServiceType::EventBridge)
            | Some(ServiceType::GuardDuty)
            | Some(ServiceType::SecurityHub)
            | Some(ServiceType::Cognito)
            | Some(ServiceType::Inspector)
            | Some(ServiceType::Backup)
            | Some(ServiceType::ServiceCatalog)
            | Some(ServiceType::NetworkFirewall)
            | Some(ServiceType::Code)
            | Some(ServiceType::Ram)
            | Some(ServiceType::TrustedAdvisor)
            | Some(ServiceType::Fsx)
            | Some(ServiceType::Bedrock)
            | Some(ServiceType::AgentCore)
            | Some(ServiceType::Kinesis)
            | Some(ServiceType::StepFunctions)
            | Some(ServiceType::Redshift)
            | Some(ServiceType::Athena)
            | Some(ServiceType::Glue)
            | Some(ServiceType::Ses)
            | Some(ServiceType::Fms)
            | Some(ServiceType::CloudFront)
            | Some(ServiceType::CloudTrail)
            | Some(ServiceType::ControlTower)
            | Some(ServiceType::S3Tables)
    );
    // Hide the top banner on the welcome splash — it has its own logo.
    let show_banner = app.banner_visible && app.current_service.is_some();
    let layout = AppLayout::new(frame.size(), show_banner, show_sub_tabs, app.layout_mode);

    // Render banner if visible
    if let Some(banner_area) = layout.banner_area {
        banner::render_banner(banner_area, frame);
    }

    // Render service tab bar
    service_tabs::render_service_tabs(app, layout.tabs_area, frame);

    // Render search bar
    search_bar::render_search_bar(app, layout.search_area, frame);

    // Render service-specific sub-tabs
    if let Some(sub_tabs_area) = layout.sub_tabs_area {
        match app.current_service {
            Some(ServiceType::EC2) => {
                ec2_tabs::render_ec2_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::ECS) => {
                ecs_tabs::render_ecs_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::IdentityCenter) => {
                idc_tabs::render_idc_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::VPC) => {
                vpc_tabs::render_vpc_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::S3) => {
                s3_tabs::render_s3_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::CloudFormation) => {
                cfn_tabs::render_cfn_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Route53) => {
                r53_tabs::render_r53_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::CloudFront) => {
                cloudfront_tabs::render_cloudfront_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::CloudWatch) => {
                cw_tabs::render_cw_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::RDS) => {
                rds_tabs::render_rds_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::IAM) => {
                iam_tabs::render_iam_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Organizations) => {
                organizations_tabs::render_org_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Elb) => {
                elb_tabs::render_elb_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Messaging) => {
                messaging_tabs::render_messaging_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Ssm) => {
                ssm_tabs::render_ssm_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Config) => {
                config_tabs::render_config_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Waf) => {
                waf_tabs::render_waf_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Cost) => {
                cost_tabs::render_cost_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::TransitGateway) => {
                tgw_tabs::render_tgw_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Route53Resolver) => {
                resolver_tabs::render_resolver_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::DirectConnect) => {
                dx_tabs::render_dx_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::ApiGateway) => {
                apigw_tabs::render_apigw_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::ServiceQuotas) => {
                sq_tabs::render_sq_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::CloudTrail) => {
                ct_tabs::render_ct_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::EventBridge) => {
                eventbridge_tabs::render_eventbridge_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Kinesis) => {
                kinesis_tabs::render_kinesis_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::StepFunctions) => {
                sfn_tabs::render_sfn_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Redshift) => {
                redshift_tabs::render_redshift_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::ControlTower) => {
                controltower_tabs::render_controltower_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Athena) => {
                athena_tabs::render_athena_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Fms) => {
                fms_tabs::render_fms_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Glue) => {
                glue_tabs::render_glue_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Ses) => {
                ses_tabs::render_ses_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::GuardDuty) => {
                gd_tabs::render_gd_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::SecurityHub) => {
                sh_tabs::render_sh_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Cognito) => {
                cognito_tabs::render_cognito_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Inspector) => {
                insp_tabs::render_insp_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Backup) => {
                backup_tabs::render_backup_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::ServiceCatalog) => {
                servicecatalog_tabs::render_servicecatalog_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::NetworkFirewall) => {
                network_firewall_tabs::render_network_firewall_tabs(
                    app,
                    sub_tabs_area,
                    frame,
                );
            }
            Some(ServiceType::Code) => {
                code_tabs::render_code_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Ram) => {
                ram_tabs::render_ram_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::TrustedAdvisor) => {
                ta_tabs::render_ta_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Fsx) => {
                fsx_tabs::render_fsx_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::S3Tables) => {
                s3tables_tabs::render_s3tables_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::Bedrock) => {
                bedrock_tabs::render_bedrock_tabs(app, sub_tabs_area, frame);
            }
            Some(ServiceType::AgentCore) => {
                agentcore_tabs::render_agentcore_tabs(app, sub_tabs_area, frame);
            }
            _ => {}
        }
    }

    if app.current_service.is_none() {
        // Nothing loaded yet — show the welcome/help splash across
        // the whole content area instead of the list + details panes.
        let rl = layout.resource_list_area;
        let d = layout.details_area;
        let content = ratatui::layout::Rect {
            x: rl.x,
            y: rl.y,
            width: (d.x + d.width).saturating_sub(rl.x),
            height: rl.height,
        };
        splash::render_splash(content, frame);
    } else {
        // Render resource list
        resource_list::render_resource_list(app, layout.resource_list_area, frame);

        // Render details pane
        details_pane::render_details_pane(app, layout.details_area, frame);
    }

    // Render status bar
    render_status_bar(app, layout.status_area, frame);

    // Service completion dropdown — rendered after content so it overlays
    search_bar::render_service_completions(app, layout.search_area, frame);

    // Render help overlay if visible (on top of everything)
    if app.help_visible {
        help_overlay::render_help_overlay(app, frame);
    }

    // Render region selector if visible
    if app.region_selector.visible {
        // Add this block
        region_selector::render_region_selector(
            &app.region_selector,
            app.current_region,
            frame,
        );
    }

    // Render profile selector if visible
    if app.profile_selector.visible {
        profile_selector::render_profile_selector(
            &app.profile_selector,
            app.aws_clients.current_profile(),
            frame,
        );
    }

    // Render the org role picker if visible
    if app.org_role_selector.visible {
        org_role_selector::render_org_role_selector(&app.org_role_selector, frame);
    }

    // Render the quota service picker if visible
    if app.quota_service_selector.visible {
        quota_service_selector::render_quota_service_selector(
            &app.quota_service_selector,
            &app.quota_service_code,
            frame,
        );
    }

    // Render the CloudTrail event-filter modal if visible
    if app.ct_filter_modal.visible {
        ct_filter_modal::render_ct_filter_modal(&app.ct_filter_modal, frame);
    }

    // Render service selector if visible
    if app.service_selector.visible {
        service_selector::render_service_selector(
            &app.service_selector,
            app.current_service,
            frame,
        );
    }

    // Render the SSM session action modal if visible
    if app.ssm_session_modal.visible {
        ssm_session_modal::render_ssm_session_modal(&app.ssm_session_modal, frame);
    }
    // The S3 object browser now renders inside the detail pane
    // (see render_details_pane).

    // Render the jump-list (navigation history) picker if visible
    if app.jump_list_visible {
        jump_list::render_jump_list(app, frame);
    }

    // Render the bookmarks picker if visible
    if app.bookmarks_visible {
        jump_list::render_bookmarks(app, frame);
    }

    // Render the message-history viewer if visible
    if app.message_history_visible {
        message_log::render_message_log(app, frame);
    }

    // Render the macro picker (or the name prompt) if visible
    if app.macro_picker_visible {
        macro_picker::render_macro_picker(app, frame);
    }

    // Metric charts, the S3 object browser, and the DynamoDB item
    // browser all render inside the detail pane now (see
    // render_details_pane), not as full-screen overlays.
}

/// Trim a `theme::hint_line` to `max_width` cells on a `  ·  ` boundary, so
/// a narrow terminal drops whole trailing hints instead of clipping one
/// mid-word against the region label ("EscUS East" at 120 columns).
fn fit_hint_line(mut line: Line<'static>, max_width: usize) -> Line<'static> {
    if line.width() <= max_width {
        return line;
    }
    let mut width = 0usize;
    let mut keep = 0usize;
    for (i, span) in line.spans.iter().enumerate() {
        if span.content == "  ·  " {
            if width <= max_width {
                keep = i;
            } else {
                break;
            }
        }
        width += span.width();
    }
    if keep > 0 {
        line.spans.truncate(keep);
    }
    line
}

fn render_status_bar(app: &App, area: ratatui::layout::Rect, frame: &mut ratatui::Frame) {
    // Left segment: transient message (error/success/loading) or contextual key hints
    let mut left_is_hints = false;
    let left: Line = if let Some(error) = &app.error_message {
        Line::from(vec![
            Span::styled(
                " ✗ ",
                Style::default()
                    .fg(theme::error())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(error.clone(), Style::default().fg(theme::error())),
            Span::styled(
                "  (y to copy)",
                Style::default().fg(theme::text_dim()),
            ),
        ])
    } else if let Some(progress) = &app.deep_export_progress {
        // An armed deep export outranks the loading/hint lines: it is the one
        // thing on screen the user is actively waiting for.
        Line::from(vec![
            Span::styled(
                format!(" {} ", theme::spinner(app.tick_count)),
                Style::default().fg(theme::accent()),
            ),
            Span::styled(progress.clone(), Style::default().fg(theme::accent())),
        ])
    } else if let Some(success) = &app.success_message {
        Line::from(vec![
            Span::styled(
                " ✓ ",
                Style::default()
                    .fg(theme::success())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(success.clone(), Style::default().fg(theme::success())),
        ])
    } else if let Some(region) = &app.switching_region {
        Line::from(Span::styled(
            format!(
                " {} Switching to {}…",
                theme::spinner(app.tick_count),
                region.display_name()
            ),
            Style::default().fg(theme::warning()),
        ))
    } else if app.show_loading_indicator() {
        let service_name = app
            .current_service
            .map(|s| s.name().to_string())
            .unwrap_or_else(|| "resources".to_string());
        let detail = match &app.loading_progress {
            Some(progress) => {
                let count = match progress.total_count {
                    Some(total) => format!(" {}/{}", progress.loaded_count, total),
                    None => format!(" {}", progress.loaded_count),
                };
                match &progress.status_message {
                    Some(msg) => format!("{} — {}", count, msg),
                    None => count,
                }
            }
            None => String::new(),
        };
        Line::from(Span::styled(
            format!(
                " {} Loading {}{}…",
                theme::spinner(app.tick_count),
                service_name,
                detail
            ),
            Style::default().fg(theme::warning()),
        ))
    } else {
        left_is_hints = true;
        let mut line = if app.search_active {
            theme::hint_line(&[("⏎", "confirm"), ("Esc", "cancel"), ("@service", "switch")])
        } else if app.details_focused {
            // Kept short and stable — the detail-pane footer carries the
            // pane-specific hints, `?` has the full reference.
            let mut hints: Vec<(&str, &str)> =
                vec![("j/k", "scroll"), ("y", "copy"), ("e", "editor")];
            if app.supports_metrics_overlay() {
                hints.push(("m", "metrics"));
            }
            if app.supports_log_tail() {
                hints.push(("t", "tail"));
            }
            if app.supports_session() {
                hints.push(("s", "session"));
            }
            hints.push(("r", "refresh"));
            hints.extend_from_slice(&[("Esc", "back"), ("?", "help")]);
            theme::hint_line(&hints)
        } else {
            let mut hints: Vec<(&str, &str)> = vec![
                ("j/k", "move"),
                ("h/l", "sub-tab"),
                ("/", "search"),
                ("⏎", "details"),
                ("y", "copy id"),
            ];
            if app.supports_metrics_overlay() {
                hints.push(("m", "metrics"));
            }
            if app.supports_log_tail() {
                hints.push(("t", "tail"));
            }
            if app.supports_object_browser() {
                hints.push(("o", "objects"));
            }
            if app.supports_item_browser() {
                hints.push(("i", "items"));
            }
            if app.supports_session() {
                hints.push(("s", "session"));
            }
            if app.supports_value_reveal() {
                hints.push(("x", "reveal"));
            }
            if app.supports_trail_lens() {
                hints.push(("W", "trail"));
            }
            if app.supports_refs_lens() {
                hints.push(("U", "refs"));
            }
            if app.supports_access_lens() {
                hints.push(("N", "access"));
            }
            if app.noise_in_view {
                hints.push(("a", if app.hide_noise { "show all" } else { "hide noise" }));
            }
            hints.push(("'", "bookmarks"));
            hints.extend_from_slice(&[("?", "help"), ("q", "quit")]);
            theme::hint_line(&hints)
        };
        line.spans.insert(0, Span::raw(" "));
        line
    };

    // Right segment: watch chip (when active), data age (when stale enough to
    // matter), region, and resource count, always visible
    let mut right_spans = Vec::new();
    // Recording / playback chips lead the right segment — the app is in a
    // mode, and that has to be visible at all times.
    if let Some(rec) = &app.macro_recorder {
        right_spans.push(Span::styled(
            format!("● REC {} steps  ", rec.steps.len()),
            Style::default()
                .fg(theme::error())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(p) = &app.macro_player {
        right_spans.push(Span::styled(
            format!(
                "▶ {} {}/{}  ",
                p.name,
                (p.index + 1).min(p.steps.len()),
                p.steps.len()
            ),
            Style::default().fg(theme::aws_orange()),
        ));
    }
    if app.watch.enabled {
        // Spinner while a watch refresh streams into staging; steady ⟳ between
        // refreshes. This is watch mode's only loading indicator — the list
        // stays populated, so the usual "Loading…" line is suppressed.
        let icon = if app.watch_refresh_active() {
            theme::spinner(app.tick_count).to_string()
        } else {
            "⟳".to_string()
        };
        right_spans.push(Span::styled(
            format!("{} watch {}s  ", icon, app.watch.interval.as_secs()),
            Style::default().fg(theme::success()),
        ));
    }
    if let Some(age) = app.current_data_age() {
        let secs = age.as_secs();
        let label = if secs >= 3600 {
            format!("⟳ {}h ago  ", secs / 3600)
        } else {
            format!("⟳ {}m ago  ", secs / 60)
        };
        right_spans.push(Span::styled(label, Style::default().fg(theme::text_dim())));
    }
    right_spans.push(Span::styled(
        app.current_region.display_name().to_string(),
        Style::default().fg(theme::aws_orange()),
    ));
    right_spans.push(Span::styled(
        format!(
            "  {}/{} ",
            app.filtered_resources.len(),
            app.resources.len()
        ),
        Style::default().fg(theme::text_dim()),
    ));
    let right = Line::from(right_spans);

    let right_width = right.width() as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(right_width)])
        .split(area);

    // Hints yield to the always-visible right segment, one column of air
    // between them; messages keep clipping (they are `y`-copyable in full).
    let left = if left_is_hints {
        fit_hint_line(left, chunks[0].width.saturating_sub(1) as usize)
    } else {
        left
    };
    frame.render_widget(Paragraph::new(left), chunks[0]);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), chunks[1]);
}

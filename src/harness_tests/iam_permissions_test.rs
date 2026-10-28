//! `⏎` on a role's Permissions rows goes to the policy: a managed row jumps
//! to the policy's pane (its Document section), an inline row — which has no
//! pane — moves to the role's Policies section, onto that document.

use super::*;
use crate::aws::services::iam::{IamPolicy, IamPolicyDoc, IamPolicyDocs, IamPrincipalKind, IamRoleDetails};

const ADMIN: &str = "arn:aws:iam::aws:policy/AdministratorAccess";

/// A role with one AWS-managed and one inline policy, both lazy bundles
/// already loaded, and the managed policy in the list beside it.
async fn role_with_policies() -> (App, mpsc::UnboundedSender<Event>, mpsc::UnboundedReceiver<Event>) {
    let (mut app, tx, rx) = test_app().await;
    let role = all_mocks()
        .into_iter()
        .find(|(_, l, _)| *l == "IamRole")
        .map(|(_, _, r)| r)
        .unwrap();
    select_mock(&mut app, ServiceType::IAM, role);
    let admin = IamPolicy::from_sdk(
        &aws_sdk_iam::types::Policy::builder()
            .arn(ADMIN)
            .policy_name("AdministratorAccess")
            .default_version_id("v1")
            .build(),
    );
    app.resources.push(Box::new(admin));
    app.lazy.iam_role_details.apply(
        "mock-role".to_string(),
        Ok(IamRoleDetails {
            managed_policies: vec![("AdministratorAccess".to_string(), ADMIN.to_string())],
            inline_policy_names: vec!["sqs-consume".to_string()],
            last_used_date: None,
            last_used_region: None,
            permissions_boundary: None,
            tags: HashMap::new(),
        }),
    );
    app.lazy.iam_policy_docs.apply(
        IamPrincipalKind::Role.docs_key("mock-role"),
        Ok(IamPolicyDocs {
            docs: vec![
                IamPolicyDoc {
                    name: "AdministratorAccess".to_string(),
                    arn: Some(ADMIN.to_string()),
                    document: Ok("{}".to_string()),
                },
                IamPolicyDoc { name: "sqs-consume".to_string(), arn: None, document: Ok("{}".to_string()) },
            ],
            omitted: 0,
        }),
    );
    app.focus_details_panel(&tx);
    app.handle_key(key(KeyCode::Char('3')), &tx).await.unwrap();
    (app, tx, rx)
}

fn section_label(app: &App) -> &'static str {
    app.get_selected_resource().unwrap().detail_sections().unwrap().sections[app.detail_section_idx].label
}

fn select_row(app: &mut App, key: &str) {
    let rows = app.get_detail_lines_filtered();
    let at = rows.iter().position(|(k, _)| k == key).unwrap_or_else(|| panic!("no row {key:?} in {rows:?}"));
    app.details_selected_index = Some(at);
}

#[tokio::test]
async fn enter_on_an_inline_row_lands_on_its_document_in_policies() {
    let (mut app, tx, _rx) = role_with_policies().await;
    assert_eq!(section_label(&app), "Permissions");
    select_row(&mut app, "  sqs-consume");
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();

    assert_eq!(section_label(&app), "Policies");
    let rows = app.get_detail_lines_filtered();
    let at = app.details_selected_index.unwrap();
    assert_eq!(rows[at].0, "sqs-consume", "cursor on the inline policy's header");
    assert_eq!(rows[at + 1], ("  Type".to_string(), "Inline".to_string()));
}

#[tokio::test]
async fn enter_on_a_managed_row_jumps_to_the_policy_document() {
    let (mut app, tx, _rx) = role_with_policies().await;
    select_row(&mut app, "  AdministratorAccess");
    app.handle_key(key(KeyCode::Enter), &tx).await.unwrap();

    assert_eq!(app.get_selected_resource_id().as_deref(), Some(ADMIN));
    assert!(app.details_focused);
    assert_eq!(section_label(&app), "Document");
    // Ctrl-O comes back to the role.
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL), &tx)
        .await
        .unwrap();
    assert_eq!(app.get_selected_resource_id().as_deref(), Some("arn:aws:iam::123456789012:role/mock-role"));
}

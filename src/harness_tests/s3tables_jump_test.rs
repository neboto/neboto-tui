//! S3 Tables ARN jumps land on the right sub-tab. Rows are ARN-keyed, and a
//! same-service jump keeps the current view unless the target carries one —
//! the table pane's Bucket row used to strand on the Tables tab.

use super::*;
use crate::app::{JumpView, S3TablesView};
use crate::aws::services::s3tables::{S3Table, S3TableBucket};

const BUCKET_ARN: &str = "arn:aws:s3tables:us-east-1:123456789012:bucket/lake";
const TABLE_ARN: &str =
    "arn:aws:s3tables:us-east-1:123456789012:bucket/lake/table/0000aaaa-1111-2222-3333-4444bbbbcccc";

fn bucket() -> S3TableBucket {
    S3TableBucket::from_sdk(
        &aws_sdk_s3tables::types::TableBucketSummary::builder()
            .arn(BUCKET_ARN)
            .name("lake")
            .owner_account_id("123456789012")
            .created_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build()
            .unwrap(),
    )
}

fn table() -> S3Table {
    S3Table::from_sdk(
        &aws_sdk_s3tables::types::TableSummary::builder()
            .namespace("sales")
            .name("orders")
            .r#type(aws_sdk_s3tables::types::TableType::Customer)
            .table_arn(TABLE_ARN)
            .created_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .modified_at(aws_smithy_types::DateTime::from_secs(1_700_000_000))
            .build()
            .unwrap(),
        BUCKET_ARN,
        "lake",
    )
}

#[test]
fn arns_carry_their_sub_tab() {
    use crate::ui::widgets::details_pane::resource_jump_target;
    let t = resource_jump_target("Bucket", BUCKET_ARN, ServiceType::S3Tables).expect("bucket");
    assert_eq!(t.id, BUCKET_ARN);
    assert!(matches!(t.view, JumpView::S3Tables(S3TablesView::Buckets)));
    // A table ARN also starts `bucket/…` — it must not route to Buckets.
    let t = resource_jump_target("Table", TABLE_ARN, ServiceType::S3Tables).expect("table");
    assert_eq!(t.id, TABLE_ARN);
    assert!(matches!(t.view, JumpView::S3Tables(S3TablesView::Tables)));
}

#[tokio::test]
async fn enter_on_a_table_bucket_row_lands_on_the_bucket() {
    let (mut app, tx, _rx) = test_app().await;
    select_mock(&mut app, ServiceType::S3Tables, Box::new(table()));
    app.resources = vec![Box::new(bucket()), Box::new(table())];
    app.s3tables_view = S3TablesView::Tables;
    app.update_search();
    app.selected_index = Some(0);
    app.details_focused = true;
    app.reset_detail_section_to_default(&tx);
    let rows = app.get_detail_lines_filtered();
    let row = rows
        .iter()
        .position(|(k, v)| k == "Bucket" && v == BUCKET_ARN)
        .unwrap_or_else(|| panic!("Bucket row in {rows:?}"));
    app.details_selected_index = Some(row);
    app.handle_event(Event::Key(key(KeyCode::Enter)), &tx).await.unwrap();

    assert_eq!(app.s3tables_view, S3TablesView::Buckets);
    assert_eq!(
        app.get_selected_resource().map(|r| r.id().to_string()).as_deref(),
        Some(BUCKET_ARN)
    );
    assert!(app.details_focused, "lands in the bucket's detail pane");
}

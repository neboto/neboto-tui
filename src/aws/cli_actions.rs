//! The `C` command catalogue: ready-to-paste AWS CLI commands for a resource.
//!
//! neboto is read-only and stays that way — nothing here is ever *run*. `C`
//! copies a command to the clipboard with the resource's ids, `--region` and
//! `--profile` filled in, and the user runs it in their own terminal. The
//! read command (`Resource::cli_command`) is always the first row; types add
//! operational commands through `Resource::cli_actions`.
//!
//! Rules for what goes in a catalogue (see issue #34):
//! - **Nothing destructive.** No terminate / delete / purge / deregister —
//!   reversible operational commands only.
//! - **Never a command that reveals a secret** (same line as `cli_command`).
//! - A command that takes a value (desired count, capacity) is prefilled with
//!   the resource's **current** value, so a blind paste is a no-op.

use crate::aws::resource::{shell_quote, Resource};

/// How much a command can do — drives grouping, colour and gating in the
/// picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CliTier {
    /// Reads state (`describe-*`, `get-console-output`).
    Inspect,
    /// Opens a session or configures a local tool (`ssm start-session`,
    /// `eks update-kubeconfig`, `ecs execute-command`). Changes nothing in
    /// the account.
    Connect,
    /// Changes the account (start/stop, force deploy, scale). Flagged ⚠ and
    /// disabled while an org role is assumed.
    Change,
}

impl CliTier {
    pub fn heading(self) -> &'static str {
        match self {
            CliTier::Inspect => "Inspect",
            CliTier::Connect => "Connect",
            CliTier::Change => "Change",
        }
    }
}

/// Batch shape of a command whose id flag takes a list
/// (`--instance-ids a b c`): `head` + the quoted ids + `tail`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Batch {
    head: String,
    id: String,
    tail: String,
}

/// One copyable command for one resource. Service-part only — the app
/// appends `--region` / `--profile` / `--endpoint-url` from the current
/// context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliAction {
    pub tier: CliTier,
    /// Short row label, conventionally the operation (`stop-instances`).
    pub label: String,
    pub command: String,
    /// Caveat shown under the preview (`needs ECS Exec enabled`).
    pub note: Option<String>,
    batch: Option<Batch>,
}

impl CliAction {
    /// A single-resource command.
    pub fn new(tier: CliTier, label: impl Into<String>, command: impl Into<String>) -> Self {
        CliAction {
            tier,
            label: label.into(),
            command: command.into(),
            note: None,
            batch: None,
        }
    }

    /// A command whose id flag takes a list, so a visual selection of several
    /// resources merges into one command. `head` ends with the flag
    /// (`aws ec2 stop-instances --instance-ids`); `id` is quoted here.
    pub fn batchable(
        tier: CliTier,
        label: impl Into<String>,
        head: impl Into<String>,
        id: &str,
        tail: impl Into<String>,
    ) -> Self {
        let head = head.into();
        let tail = tail.into();
        let id = shell_quote(id);
        CliAction {
            tier,
            label: label.into(),
            command: format!("{} {}{}", head, id, tail),
            note: None,
            batch: Some(Batch { head, id, tail }),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// A picker row: an action with the context flags appended and its gate
/// resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliPickerRow {
    pub tier: CliTier,
    pub label: String,
    /// The exact text `⏎` copies.
    pub command: String,
    pub note: Option<String>,
    /// Why the row can't be copied right now (None = copyable).
    pub disabled: Option<String>,
}

/// The open `C` picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliPickerState {
    /// What the commands target (`i-0abc (web-1)`, `3 instances`).
    pub subject: String,
    pub rows: Vec<CliPickerRow>,
    pub selected: usize,
}

impl CliPickerState {
    pub fn selected_row(&self) -> Option<&CliPickerRow> {
        self.rows.get(self.selected)
    }

    pub fn next(&mut self) {
        if !self.rows.is_empty() {
            self.selected = (self.selected + 1).min(self.rows.len() - 1);
        }
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

/// Where a copied command should point: appended to every row.
#[derive(Debug, Clone, Default)]
pub struct CliContext {
    pub region: String,
    /// None while an org role is assumed (no flag reproduces that session)
    /// or when running on default credentials.
    pub profile: Option<String>,
    pub endpoint_url: Option<String>,
    /// An org role is assumed: Change rows are disabled.
    pub assumed_role: bool,
}

impl CliContext {
    fn suffix(&self) -> String {
        let mut s = format!(" --region {}", self.region);
        if let Some(p) = &self.profile {
            s.push_str(&format!(" --profile {}", shell_quote(p)));
        }
        // An emulator session must say so: a pasted `stop-instances` meant for
        // LocalStack would otherwise run against the real account.
        if let Some(e) = &self.endpoint_url {
            s.push_str(&format!(" --endpoint-url {}", shell_quote(e)));
        }
        s
    }
}

/// Reason Change rows are gated while an org role is assumed.
pub const ASSUMED_ROLE_REASON: &str =
    "an org role is assumed — no --profile can reproduce it, so a pasted change would hit whichever account your shell points at";

/// The label of a read command: its operation (`aws ec2 describe-instances …`
/// → `describe-instances`).
fn op_label(command: &str) -> String {
    command
        .split_whitespace()
        .nth(2)
        .unwrap_or(command)
        .to_string()
}

/// Every command for one resource: the read command first, then the type's
/// catalogue in tier order (an action identical to the read command is
/// dropped, so a type can declare its read command batchable without it
/// showing twice).
pub fn single_actions(resource: &dyn Resource) -> Vec<CliAction> {
    let mut out = Vec::new();
    let read = resource.cli_command();
    if let Some(cmd) = &read {
        out.push(CliAction::new(CliTier::Inspect, op_label(cmd), cmd.clone()));
    }
    let mut extra: Vec<CliAction> = resource
        .cli_actions()
        .into_iter()
        .filter(|a| Some(&a.command) != read.as_ref())
        .collect();
    extra.sort_by_key(|a| a.tier); // stable: keeps the type's order within a tier
    out.extend(extra);
    out
}

/// Commands for a multi-resource selection: the batchable actions **every**
/// selected resource offers (matched by tier + label + batch head/tail), with
/// their ids merged into one list. Non-batchable actions (a per-service
/// `update-service`) can't be merged, so they're left out.
pub fn batch_actions(resources: &[&dyn Resource]) -> Vec<CliAction> {
    let Some((first, rest)) = resources.split_first() else {
        return Vec::new();
    };
    let per: Vec<Vec<CliAction>> = rest.iter().map(|r| r.cli_actions()).collect();
    let mut out = Vec::new();
    for action in first.cli_actions() {
        let Some(batch) = &action.batch else { continue };
        let mut ids = vec![batch.id.clone()];
        let all_have = per.iter().all(|actions| {
            match actions.iter().find(|a| {
                a.tier == action.tier
                    && a.label == action.label
                    && a.batch.as_ref().is_some_and(|b| b.head == batch.head && b.tail == batch.tail)
            }) {
                Some(a) => {
                    let id = &a.batch.as_ref().expect("matched on batch").id;
                    if !ids.contains(id) {
                        ids.push(id.clone());
                    }
                    true
                }
                None => false,
            }
        });
        if all_have {
            let mut merged = action.clone();
            merged.command = format!("{} {}{}", batch.head, ids.join(" "), batch.tail);
            merged.batch = None;
            out.push(merged);
        }
    }
    out.sort_by_key(|a| a.tier);
    out
}

/// Turn actions into picker rows for the given context.
pub fn picker_rows(actions: Vec<CliAction>, ctx: &CliContext) -> Vec<CliPickerRow> {
    let suffix = ctx.suffix();
    actions
        .into_iter()
        .map(|a| CliPickerRow {
            disabled: (a.tier == CliTier::Change && ctx.assumed_role)
                .then(|| ASSUMED_ROLE_REASON.to_string()),
            tier: a.tier,
            label: a.label,
            command: format!("{}{}", a.command, suffix),
            note: a.note,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> CliContext {
        CliContext {
            region: "ap-southeast-2".into(),
            profile: Some("prod".into()),
            endpoint_url: None,
            assumed_role: false,
        }
    }

    #[test]
    fn batchable_quotes_and_builds_single_command() {
        let a = CliAction::batchable(
            CliTier::Change,
            "stop-instances",
            "aws ec2 stop-instances --instance-ids",
            "i-1",
            "",
        );
        assert_eq!(a.command, "aws ec2 stop-instances --instance-ids i-1");
    }

    #[test]
    fn rows_append_context_and_gate_changes_under_assumed_role() {
        let actions = vec![
            CliAction::new(CliTier::Inspect, "describe", "aws x describe"),
            CliAction::new(CliTier::Change, "stop", "aws x stop"),
        ];
        let rows = picker_rows(actions.clone(), &ctx());
        assert_eq!(rows[1].command, "aws x stop --region ap-southeast-2 --profile prod");
        assert!(rows.iter().all(|r| r.disabled.is_none()));

        let assumed = CliContext { profile: None, assumed_role: true, ..ctx() };
        let rows = picker_rows(actions, &assumed);
        assert!(rows[0].disabled.is_none(), "reads stay copyable");
        assert!(rows[1].disabled.is_some(), "changes are gated");
        assert_eq!(rows[1].command, "aws x stop --region ap-southeast-2");
    }

    #[test]
    fn endpoint_url_is_carried_into_the_command() {
        let c = CliContext { endpoint_url: Some("http://localhost:4566".into()), ..ctx() };
        let rows = picker_rows(vec![CliAction::new(CliTier::Change, "stop", "aws x stop")], &c);
        assert!(rows[0].command.ends_with("--endpoint-url http://localhost:4566"));
    }

    #[test]
    fn op_label_is_the_operation() {
        assert_eq!(op_label("aws ec2 describe-instances --instance-ids i-1"), "describe-instances");
    }
}

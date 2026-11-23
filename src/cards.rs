//! Info cards (`i`): a short page per sub-tab on what the service is, its
//! key ideas, what neboto shows of it and where the AWS docs explain the
//! rest. The text is Markdown in `docs/cards/` (its README is the format),
//! built into the binary; this module maps sub-tabs to cards and parses
//! the small Markdown subset into [`Block`]s. The popup is
//! `src/ui/widgets/info_card.rs`. A card is text only: nothing is fetched,
//! and its links are opened or copied only on a key press.
//!
//! Ported from nebaz (`src/cards.rs`, nebaz PR #216), which has a card for
//! every sub-tab. neboto starts with a few to see whether the format earns
//! the rest, so a tab without one says so instead of failing a test.

use crate::app::{Ec2View, EcsView, IamView, JumpView, OrgView};
use crate::aws::service::ServiceType;
use std::cell::Cell;

pub struct Card {
    /// The popup's title: the tab (or tabs) the card is about.
    pub label: &'static str,
    /// Whether the card is about this place. A sub-tab service matches on
    /// its view; a single-list service (Cost) on the service alone.
    pub at: fn(ServiceType, &JumpView) -> bool,
    pub source: &'static str,
}

pub static CARDS: &[Card] = &[
    Card {
        label: "Security Groups",
        at: |_, v| matches!(v, JumpView::Ec2(Ec2View::SecurityGroups)),
        source: include_str!("../docs/cards/security-groups.md"),
    },
    Card {
        label: "IAM Roles · Policies",
        at: |_, v| matches!(v, JumpView::Iam(IamView::Roles | IamView::Policies)),
        source: include_str!("../docs/cards/iam-roles-policies.md"),
    },
    Card {
        label: "ECS",
        at: |_, v| matches!(v, JumpView::Ecs(EcsView::Clusters | EcsView::Services | EcsView::TaskDefinitions | EcsView::Tasks)),
        source: include_str!("../docs/cards/ecs.md"),
    },
    Card { label: "Cost", at: |s, _| s == ServiceType::Cost, source: include_str!("../docs/cards/cost.md") },
    Card {
        label: "Organization Policies",
        at: |_, v| matches!(v, JumpView::Org(OrgView::Policies)),
        source: include_str!("../docs/cards/org-policies.md"),
    },
];

/// The index into [`CARDS`] of the card for this place, if one is written.
pub fn for_place(service: ServiceType, view: &JumpView) -> Option<usize> {
    CARDS.iter().position(|c| (c.at)(service, view))
}

/// The open card: which one, the cursor and selection over its blocks, and
/// the first line on screen (the draw keeps the cursor in view).
pub struct InfoCardState {
    pub card: usize,
    /// The block under the cursor (a paragraph, a bullet, a link).
    pub cursor: usize,
    /// Where `V` started a selection.
    pub anchor: Option<usize>,
    pub scroll: Cell<u16>,
}

impl InfoCardState {
    pub fn new(card: usize) -> Self {
        Self { card, cursor: 0, anchor: None, scroll: Cell::new(0) }
    }

    pub fn card(&self) -> &'static Card {
        &CARDS[self.card]
    }

    /// The selected blocks, or the cursor's alone.
    pub fn range(&self) -> (usize, usize) {
        let a = self.anchor.unwrap_or(self.cursor);
        (a.min(self.cursor), a.max(self.cursor))
    }
}

/// One piece of a card, in reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Title(String),
    Heading(String),
    Paragraph(String),
    Bullet(String),
    /// A `- Label: https://…` line, numbered in the card's order.
    Link { n: usize, label: String, url: String },
}

impl Card {
    /// The card as blocks. A paragraph's lines join with spaces; a bullet
    /// whose text is `Label: https://…` is a link.
    pub fn blocks(&self) -> Vec<Block> {
        let mut out = Vec::new();
        let mut para: Vec<&str> = Vec::new();
        let mut links = 0;
        let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
            if !para.is_empty() {
                out.push(Block::Paragraph(para.join(" ")));
                para.clear();
            }
        };
        for line in self.source.lines().map(str::trim_end) {
            if line.trim().is_empty() {
                flush(&mut para, &mut out);
            } else if let Some(t) = line.strip_prefix("## ") {
                flush(&mut para, &mut out);
                out.push(Block::Heading(t.trim().to_string()));
            } else if let Some(t) = line.strip_prefix("# ") {
                flush(&mut para, &mut out);
                out.push(Block::Title(t.trim().to_string()));
            } else if let Some(t) = line.strip_prefix("- ") {
                flush(&mut para, &mut out);
                match t.split_once(": https://") {
                    Some((label, rest)) => {
                        links += 1;
                        out.push(Block::Link {
                            n: links,
                            label: label.trim().to_string(),
                            url: format!("https://{}", rest.trim()),
                        });
                    }
                    None => out.push(Block::Bullet(t.trim().to_string())),
                }
            } else {
                para.push(line.trim());
            }
        }
        flush(&mut para, &mut out);
        out
    }

    /// Blocks `lo..=hi` as plain text for the clipboard: the card's own
    /// Markdown minus the `**`, a blank line before each heading and
    /// paragraph, so a pasted range reads as it does on screen.
    pub fn text_of(&self, lo: usize, hi: usize) -> String {
        let mut out = String::new();
        for (i, b) in self.blocks().iter().enumerate().skip(lo).take(hi.saturating_sub(lo) + 1) {
            let (gap, line) = match b {
                Block::Title(t) => (false, t.clone()),
                Block::Heading(h) => (true, h.clone()),
                Block::Paragraph(p) => (true, p.replace("**", "")),
                Block::Bullet(t) => (false, format!("- {}", t.replace("**", ""))),
                Block::Link { label, url, .. } => (false, format!("- {}: {}", label, url)),
            };
            if i > lo {
                out.push_str(if gap { "\n\n" } else { "\n" });
            }
            out.push_str(&line);
        }
        out
    }

    /// The URL of link `n` (1-based).
    pub fn link(&self, n: usize) -> Option<String> {
        self.blocks().into_iter().find_map(|b| match b {
            Block::Link { n: k, url, .. } if k == n => Some(url),
            _ => None,
        })
    }
}

/// `text` as words, each a run of `(bold, piece)` segments: `**` toggles
/// emphasis without breaking a word, so `**Ingress**:` stays one word.
pub fn styled_words(text: &str) -> Vec<Vec<(bool, String)>> {
    let mut words = Vec::new();
    let mut word: Vec<(bool, String)> = Vec::new();
    let mut bold = false;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("**") {
            bold = !bold;
            rest = after;
            continue;
        }
        rest = &rest[c.len_utf8()..];
        if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        match word.last_mut() {
            Some((b, piece)) if *b == bold => piece.push(c),
            _ => word.push((bold, c.to_string())),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_card_has_a_title_the_three_parts_and_aws_links() {
        for card in CARDS {
            let blocks = card.blocks();
            assert!(matches!(blocks.first(), Some(Block::Title(_))), "{}: starts with a # title", card.label);
            let headings: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Heading(h) => Some(h.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(headings, ["Key ideas", "In neboto", "Learn more"], "{}", card.label);
            let links: Vec<&str> = blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Link { url, .. } => Some(url.as_str()),
                    _ => None,
                })
                .collect();
            assert!(!links.is_empty() && links.len() <= 9, "{}: 1 to 9 links (keys 1–9 copy them)", card.label);
            assert!(
                links.iter().all(|u| u.starts_with("https://docs.aws.amazon.com/")),
                "{}: links go to the AWS docs",
                card.label
            );
        }
    }

    #[test]
    fn cards_use_only_the_markdown_the_popup_draws() {
        // No inline code, links or images: the popup would print the marks.
        for card in CARDS {
            for mark in ["`", "](", "<"] {
                assert!(!card.source.contains(mark), "{}: {mark:?} isn't part of the card format", card.label);
            }
        }
    }

    #[test]
    fn each_place_has_one_card() {
        let places: &[(ServiceType, JumpView)] = &[
            (ServiceType::EC2, JumpView::Ec2(Ec2View::SecurityGroups)),
            (ServiceType::IAM, JumpView::Iam(IamView::Roles)),
            (ServiceType::IAM, JumpView::Iam(IamView::Policies)),
            (ServiceType::ECS, JumpView::Ecs(EcsView::Tasks)),
            (ServiceType::Cost, JumpView::None),
            (ServiceType::Organizations, JumpView::Org(OrgView::Policies)),
        ];
        for (s, v) in places {
            let n = CARDS.iter().filter(|c| (c.at)(*s, v)).count();
            assert_eq!(n, 1, "{s:?} {v:?}");
        }
        assert_eq!(for_place(ServiceType::EC2, &JumpView::Ec2(Ec2View::Instances)), None);
        assert_eq!(for_place(ServiceType::IAM, &JumpView::Iam(IamView::Users)), None);
    }

    #[test]
    fn a_card_parses_into_blocks_and_numbers_its_links() {
        let card = Card {
            label: "T",
            at: |_, _| false,
            source: "# T\n\nOne line\nand the next.\n\n## Key ideas\n\n- **A**: b\n\n## Learn more\n\n- Overview: https://docs.aws.amazon.com/x\n- More: https://docs.aws.amazon.com/y\n",
        };
        assert_eq!(
            card.blocks(),
            [
                Block::Title("T".into()),
                Block::Paragraph("One line and the next.".into()),
                Block::Heading("Key ideas".into()),
                Block::Bullet("**A**: b".into()),
                Block::Heading("Learn more".into()),
                Block::Link { n: 1, label: "Overview".into(), url: "https://docs.aws.amazon.com/x".into() },
                Block::Link { n: 2, label: "More".into(), url: "https://docs.aws.amazon.com/y".into() },
            ]
        );
        assert_eq!(card.link(2).as_deref(), Some("https://docs.aws.amazon.com/y"));
        assert_eq!(card.link(3), None);
        assert_eq!(card.text_of(2, 3), "Key ideas\n- A: b");
        assert_eq!(card.text_of(0, 1), "T\n\nOne line and the next.");
        assert_eq!(card.text_of(5, 5), "- Overview: https://docs.aws.amazon.com/x");
        assert_eq!(
            styled_words("**Ingress**: in"),
            [vec![(true, "Ingress".to_string()), (false, ":".to_string())], vec![(false, "in".to_string())]]
        );
    }
}

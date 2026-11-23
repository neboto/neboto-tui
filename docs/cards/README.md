# Info cards

What `i` shows: a short card about the current sub-tab (or about a group
of sub-tabs that are the same idea). `src/cards.rs` builds them into the
binary from this folder, and its `CARDS` table maps tabs to cards. A tab
without a card says so. This is a start: nebaz has a card for every
sub-tab, while neboto has a few, to see whether the format earns the rest.

A card is short. Four parts, in this order:

1. `# Title`, then two or three sentences: what the service is.
2. `## Key ideas`: a few bullets, `**term**: what it means`.
3. `## In neboto`: what the tab shows, what its states mean, and what is
   never read and why. Take it from `docs/SERVICES.md`, which stays the
   reference. When that changes, change the card too.
4. `## Learn more`: `- Label: https://docs.aws.amazon.com/…` lines. The
   card numbers them; `o` opens the one under the cursor and `1`–`9` copy
   one. The AWS docs are the authority on AWS itself. Keep the prose here
   short so it can't drift far.

Use only this much Markdown: `#`, `##`, `- ` bullets, `**bold**` and
paragraphs. Every card has the same structure as nebaz's (whose card
sections are "In nebaz"), so a card can move between the two with only
that heading and its links changed.

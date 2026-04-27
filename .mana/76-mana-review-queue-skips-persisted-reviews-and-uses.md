---
id: '76'
title: mana review queue skips persisted reviews and uses checkpoint diff evidence with empty per-unit fallback
slug: mana-review-queue-skips-persisted-reviews-and-uses
status: open
priority: 3
created_at: '2026-04-09T18:07:35.312013Z'
updated_at: '2026-04-09T18:07:35.312013Z'
labels:
- fact
verify: cd /Users/asher/mana && cargo test -p mana-review && cargo test -p mana-cli review
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- mana/crates/mana-review/src/queue.rs
- mana/crates/mana-review/src/state.rs
- mana/crates/mana-review/src/diff.rs
- mana/crates/mana-cli/src/commands/review_human.rs
- mana/crates/mana-cli/src/commands/close/failure.rs
---

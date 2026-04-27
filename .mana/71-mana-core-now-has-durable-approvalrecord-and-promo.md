---
id: '71'
title: mana-core now has durable ApprovalRecord and PromotionRecord schema with mandatory review lineage validation
slug: mana-core-now-has-durable-approvalrecord-and-promo
status: open
priority: 3
created_at: '2026-04-09T13:26:51.160937Z'
updated_at: '2026-04-09T13:26:51.160937Z'
labels:
- fact
verify: cd /Users/asher/tower && rg -q 'struct ApprovalRecord' mana/crates/mana-core/src && rg -q 'struct PromotionRecord' mana/crates/mana-core/src && rg -q 'enum ReviewGateOutcome' mana/crates/mana-core/src && rg -q 'fn validate\(&self\) -> Result<\(\), String>' mana/crates/mana-core/src/unit/types.rs && cargo test -p mana-core approval_record --lib && cargo test -p mana-core promotion_record --lib && cargo check -p mana-core
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- mana/crates/mana-core/src/unit/types.rs
- docs/rebuild/approval-and-promotion-records.md
- docs/rebuild/review-gating-policy.md
- docs/rebuild/evidence-bundle-minimum.md
---

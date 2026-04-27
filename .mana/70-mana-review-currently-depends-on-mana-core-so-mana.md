---
id: '70'
title: mana-review currently depends on mana-core, so mana-core cannot directly embed mana-review types without creating a crate cycle
slug: mana-review-currently-depends-on-mana-core-so-mana
status: open
priority: 3
created_at: '2026-04-09T13:26:27.753248Z'
updated_at: '2026-04-09T13:26:27.753248Z'
labels:
- fact
verify: cd /Users/asher/tower && rg -q '^mana-core = \{ path = "\.\./mana-core"' mana/crates/mana-review/Cargo.toml && ! rg -q '^mana-review\s*=' mana/crates/mana-core/Cargo.toml
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- mana/crates/mana-review/Cargo.toml
- mana/crates/mana-core/Cargo.toml
---

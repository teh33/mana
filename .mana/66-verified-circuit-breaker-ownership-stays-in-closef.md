---
id: '66'
title: Verified circuit breaker ownership stays in close/failure handling, not mana-pool dispatch
slug: verified-circuit-breaker-ownership-stays-in-closef
status: open
priority: 3
created_at: '2026-04-09T12:32:18.771941Z'
updated_at: '2026-04-09T12:32:18.771941Z'
labels:
- fact
verify: cd /Users/asher/mana && rg -q 'check_circuit_breaker|CircuitBreakerResult' crates/mana-core/src/ops/close.rs crates/mana-cli/src/commands/close/failure.rs && ! rg -q 'check_circuit_breaker|CircuitBreakerResult' crates/mana-pool/src/dispatch.rs
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- mana/crates/mana-core/src/ops/close.rs
- mana/crates/mana-cli/src/commands/close/failure.rs
- mana/crates/mana-pool/src/dispatch.rs
---

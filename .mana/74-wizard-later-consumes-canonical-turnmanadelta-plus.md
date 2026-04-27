---
id: '74'
title: Wizard later consumes canonical TurnManaDelta plus direct mana graph reads and remains optional for the first version
slug: wizard-later-consumes-canonical-turnmanadelta-plus
status: open
priority: 3
created_at: '2026-04-09T17:06:50.863502Z'
updated_at: '2026-04-09T17:06:50.863502Z'
labels:
- fact
verify: cd /Users/asher/tower && test -f docs/rebuild/wizard-mana-review-queue.md && rg -q 'TurnManaDelta' docs/rebuild/wizard-mana-review-queue.md && rg -q 'review queue' docs/rebuild/wizard-mana-review-queue.md && rg -q 'focus room' docs/rebuild/wizard-mana-review-queue.md && rg -q 'direct mana graph reads' docs/rebuild/wizard-mana-review-queue.md && rg -q 'not required for the first version' docs/rebuild/wizard-mana-review-queue.md
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- docs/rebuild/wizard-mana-review-queue.md
- docs/rebuild/mana-turn-delta-contract.md
- wizard/app/desktop/src/cards.ts
- wizard/app/desktop/src/canvas.ts
- wizard/app/desktop/src/focus-room.ts
- wizard/app/desktop/src/review.ts
- wizard/app/desktop/src/runtime.ts
---

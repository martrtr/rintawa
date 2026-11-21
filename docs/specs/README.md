# Core specifications

[Minimal Core contract](core-mvp-contract.md) defines the accepted target architecture,
D1–D5 amendments, alternatives, invariants, limitations and acceptance scenarios.
[MVP API and client boundaries](mvp-api-and-client-boundaries.md) completes the
P01–P04 design: D6–D7, exact guest interfaces, browser protocol, native
release scope, initial budgets and additional acceptance checks. Read both;
the companion's explicit choices supersede earlier open alternatives.

**Status: accepted design, implementation pending.** On 2026-11-21 the owner
accepted D1–D7 subject to UI using general extension interaction without new
UI-specific Core code. The companion records this boundary and its acceptance
checks. The follow-up browser review adds **D8 as a proposed amendment**, not yet accepted:
nonblocking queries/polled queryables for all execution targets. DockView belongs
to the rtwKit shell. The comparison with official zenoh-ts/remote-api and B10–B11
checks are included in the companion; approve D8 before implementing that slice.
Only the project owner may accept further architectural changes.
The baseline is `fb1072a2c9169af1b36ea9cdffed31c146ed0341`, inspected on
2026-11-21. No builds, tests or benchmarks were run for that investigation.

This directory contains durable project specifications in English. Revision-specific
audit notes and the working refactoring checklist are local engineering material
in the ignored `.vscode/architecture/` directory; public specifications must not
depend on those private files. Older architecture/roadmap descriptions may lag
behind the current code and must not be used to restore removed systems.

Core readiness, product MVP and release readiness are separate milestones.
Inspect extension consumers early; implement the shared foundation first, then
validate the first real product workflow in rtwKit. Completing every cleanup
before that integration is not required.

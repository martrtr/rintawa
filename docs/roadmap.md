# Roadmap

Rintawa is working toward the first `0.0.1` end-to-end MVP: import a character,
create a persistent World, talk through world-scoped Chat, receive an AI-controlled reply,
restart the Host and continue the same session.

## Completed foundation

The current implementation already includes:

- immutable RTW/CAS storage and scoped package composition;
- authoritative World state, SQLite persistence, authority and transactions;
- durable WorldEvents, effect/outbox execution and multi-World runtime supervision;
- scoped `Single`/`Multiple` service routing and provider policy;
- renderer-neutral Portable UI and focused-world presentation;
- World Manager and shared Manage workspace;
- bounded file/resource ingress and streaming asset upload;
- Tavern V2/V3 Character import through `rintawa.world.creator@1`;
- rollback-safe Tavern -> playable World -> greeting -> Chat bootstrap;
- world-scoped Chat with conversations, branches, editing, deletion and attachments;
- restart persistence for the current World/Character/Chat flow.

## 0.0.1 blockers

The remaining MVP work is:

1. **O(1) idle presentation polling** — per-layer generation and Web Runtime idle-skip
   are implemented and tested; verify exact installed RTW activation before closure.
2. **Aggregate Portable UI budget** — admission-time snapshot, visible UI Layer, and
   total mounted-snapshot JSON byte limits are implemented and tested; verify
   exact installed runtime behavior before marking the blocker closed.
3. **Generic outbound HTTP** — bounded owner-scoped HTTPS request/stream support with
   timeout and network policy.
4. **Generic asynchronous operations** — bounded owner-scoped streams, terminal
   states, cancellation, short retention and single-reader service-caller handoff
   are implemented. Durable-job linkage and independent provider/consumer fixture
   acceptance remain open.
5. **Provider-neutral inference** — versioned inference contract plus one AI Provider.
6. **Narrator** — world-scoped event -> context -> inference -> authoritative NPC command.
7. **Final restart E2E** — verify the complete Tavern -> World -> Chat -> AI reply -> restart
   path, including cancellation/failure and duplicate-safe retry.

## After 0.0.1

Not required for the first MVP:

- general UI rearrangement/tiling editor;
- richer World bundle, export, fork and archive workflows;
- marketplace and remote trust/reputation;
- advanced psychology, memory, agents and embeddings;
- multiplayer, federation and distributed consensus;
- 3D/Minecraft presentation adapters.

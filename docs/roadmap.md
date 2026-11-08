# Roadmap

Rintawa is working toward the first `0.0.1` end-to-end MVP: import a character,
create a persistent World, talk through world-scoped Chat, receive an AI-controlled reply,
restart the Host and continue the same session.

## Native foundation and historical prototypes

The following subsystems and earlier prototype paths are present in the repository.
Old imported RTW UI/Chat packages are **not** verified working after the breaking
World and interaction API cuts. The foundation and old integration code include:

- immutable RTW/CAS storage and scoped package composition;
- authoritative item/property World state, SQLite persistence and atomic change sets;
- exact instance/component binding for World System service execution, including
  same-ID impostor rejection; World-schema owners sharing one logical ExtensionId
  across multiple installed instances are refused until finer property ownership exists;
- one bounded single-writer command queue per World and multi-World runtime supervision;
- a repeatable clean CLI home smoke test (`world-create`, `world-list`, `world-info`,
  `run --once`) and Host restart tests including an item-owned CAS `AssetRef` and
  relation persistence. These do not prove the later Chat/Narrator end-to-end MVP;
- scoped `Single`/`Multiple` service routing and provider policy;
- the old Portable UI runtime, UI Layer and World-focus SDK/WIT/Host stack have
  been retired; no replacement transport is integrated;
- historical World Manager and shared Manage workspace packages (not currently
  compatible with the reduced UI ABI);
- bounded file/resource ingress and streaming asset upload;
- earlier Tavern/Character and Chat integration work that requires migration to
  the replacement extension interaction and UI protocols before it can be used.
  These old package paths are historical, not verified end-to-end functionality.

The World model itself is deliberately small and does not promise more: one database per
World with exactly four tables (`world`, `items`, `property_registry`, `properties`),
UUIDv7 BLOB(16) identities, flat versioned property names, and one bounded single-writer
queue with atomic change sets. There is no durable command ledger or receipt, no event log
or replay, no job/outbox table, and no actor control grant. The old World signal
bridge has been removed; generic Engine event callbacks are in-process only and
do not provide authoritative World-change delivery.

## 0.0.1 blockers

The remaining MVP work is:

1. **Extension transport proof** — compare the proposed Zenoh integration with
   a simpler direct option if required. Prove native/WASM query/reply, pub/sub,
   authenticated admission, strict World-scope boundaries, fan-out and lifecycle
   cleanup before adopting it as Core transport.
2. **Extension-owned UI composition** — rebuild one narrow Chat/Web UI path
   using the proven transport. Portable UI generation, surface budgets and
   the former idle presentation API were removed rather than preserved.
3. **Generic outbound HTTP** — bounded owner-scoped HTTPS request/stream support with
   timeout and network policy.
4. **Generic asynchronous operations** — bounded owner-scoped streams, terminal
   states, cancellation, short retention and single-reader service-caller handoff
   are implemented and covered by a real two-component WASM service fixture.
   Durable World effect handlers and their deferred continuations were removed with
   the four-table World model. The dedicated World signal bridge was also removed;
   a replacement interaction transport has not been integrated.
5. **Provider-neutral inference** — versioned inference contract plus one AI Provider.
6. **Narrator** — a planned extension-owned request/reply path to inference and an
   authoritative World command, once the replacement interaction transport is proved.
7. **Final restart E2E** — verify the complete Tavern -> World -> Chat -> AI reply -> restart
   path, including cancellation and failure handling.

## After 0.0.1

Not required for the first MVP:

- general UI rearrangement/tiling editor;
- richer World bundle, export, fork and archive workflows;
- marketplace and remote trust/reputation;
- advanced psychology, memory, agents and embeddings;
- multiplayer, federation and distributed consensus;
- 3D/Minecraft presentation adapters.

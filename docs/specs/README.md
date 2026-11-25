# Rintawa specifications

Rintawa provides World storage, extension execution, authority and transport.
Product behavior belongs to extensions. A specification describes a contract;
it is not evidence that the product implements or passes it.

## Existing contracts

| Subject | Read |
| --- | --- |
| Minimal Core, ownership, atomic writes and resource lifecycle | [Core contract](core-mvp-contract.md) |
| Host/WASM APIs, nonblocking transport and client boundaries | [API and client boundaries](mvp-api-and-client-boundaries.md) |
| Stable browser panels and layout restoration | [Browser layout](browser-layout-amendment.md) |
| Live permissions, replaceable renderers and isolated UI contributions | [Live permissions and UI](live-permissions-and-ui-contracts.md) |
| Shared AI configuration and extension settings | [Settings and AI connections](extension-settings-and-ai-connections.md) |
| Local credential storage and access | [Secrets](host-secret-configuration.md) |
| Portable World snapshots, assets and creation | [World templates](world-template-packages.md) |
| Bounded user-resource transfer | [Resource transfer](user-resource-transfer-amendment.md) |
| Monotonic guest time | [Clock](guest-monotonic-time-amendment.md) |
| Original create→import→chat scenario | [Story delivery](mvp-story-delivery.md) |

Read each document's status and later amendments. Earlier decisions D1–D8 do not
automatically approve new proposals. The browser shell, Chat, Character and Narrator
now have implementations in the companion rtwKit repository; the former statement
that all browser/DockView work remained pending is obsolete. Their exact feature
protocols live beside those implementations.

## Proposed next product contracts

- [Concurrent AI connections and streaming](ai-multiple-providers-streaming.md): request-scoped model choice, bounded streaming and consumer admission.

- [Responsive, composable experience](extensible-experience.md): freshness,
  contextual actions, workspace layout and persistent permission experience.
- [Narrative worlds and companions](narrative-and-companion.md): ownership,
  memory/knowledge, multi-actor turns, media/native boundaries, functional coverage
  and delivery gates for the Soul-of-Waifu reference scope.

These proposals do not change accepted contracts until the owner accepts them.
They distinguish extension work from missing native/media/client guarantees.
They do not turn Character or UI into Core concepts.

## Documentation policy

Keep durable contracts public, readable without private notes, and in English.
Keep revision-specific audits, agent prompts, task queues, temporary evidence paths
and session transcripts in local engineering documentation. Do not move a useful
public contract to private storage merely because its first reader was an agent.

Some original specifications contain implementation history. Treat that history as
dated evidence, not current status; extract it when editing the relevant document,
without losing accepted decisions or breaking links. Public architecture/roadmap
texts describing retired systems are not instructions to restore them.

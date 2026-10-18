# Roadmap

The first Rintawa milestone is a working `0.0.1` experience: import a character,
create a persistent World, use world-scoped Chat, receive an AI-generated reply,
and continue the same session after restarting the Host.

## Available in `dev`

- RTW package storage, extension execution and scoped composition.
- Authoritative World commands, events and SQLite persistence.
- Renderer-neutral Portable UI and World presentation.
- Standard World Manager, Character Library and Chat packages in
  [rtwKit](https://github.com/martrtr/rtwKit).
- Tavern V2/V3 character import and persistent World/Chat sessions.

## In progress

- Generic asynchronous HTTP, operation streams, UI presentation limits and
  deferred World effect handling are implemented on
  [`feat/mvp-runtime-infrastructure`](https://github.com/martrtr/rintawa/tree/feat/mvp-runtime-infrastructure).
  Integration and final validation are tracked in [issue #5](https://github.com/martrtr/rintawa/issues/5).
- Provider-neutral inference and a world-scoped Narrator are being developed
  as independent [rtwKit](https://github.com/martrtr/rtwKit) packages.

## Before `0.0.1`

- Integrate and review the runtime infrastructure with compatible RTW packages.
- Complete the Chat, inference and Narrator interaction within a World.
- Verify the full flow on a clean install, including failure recovery and
  persistence across restarts.

Current work is tracked in [Rintawa Issues](https://github.com/martrtr/rintawa/issues)
and [rtwKit Issues](https://github.com/martrtr/rtwKit/issues).

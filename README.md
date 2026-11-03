# Rintawa

Rintawa is a local-first runtime for persistent Worlds composed from immutable RTW
packages. Core provides generic package execution, scoped composition, authoritative World
state and renderer-neutral UI infrastructure; product features are extensions.

Standard packages and the Web experience live in
[rtwKit](https://github.com/martrtr/rtwKit).

## Quick start

```bash
rintawa install ./package.rtw
rintawa list
rintawa disable <subject>
rintawa enable <subject>
rintawa run
```

RTW artifacts are validated and stored immutably by SHA-256. Global tools run in the
baseline composition; new Worlds are materialized from an independent default-world recipe
and then keep their own exact composition.

## World state

Each World is one SQLite database with exactly four domain tables: `world`, `items`,
`property_registry` and `properties`. Objects and relations are both items with UUIDv7
BLOB(16) identities, and all feature data is a versioned property value on an item.
Property names are flat identifiers such as `character.identity`; `player`, `npc` and
similar concepts are ordinary extension properties with no Core ownership semantics.

The World model deliberately keeps a reduced set of guarantees:

- one bounded single-writer command queue per World; a System proposal is validated
  against the persisted property vocabulary and committed as one atomic change set;
- one World per database, with no durable command ledger or receipts, so a repeated
  command is evaluated and committed again;
- no event log, no replay and no consumer cursors;
- no durable jobs or outbox: post-COMMIT runtime signals are best-effort in-process
  notifications and are lost if the process dies between COMMIT and dispatch;
- no actor records or control grants; authority stays Host composition policy.

## Documentation

- [Architecture](docs/architecture.md)
- [0.0.1 roadmap](docs/roadmap.md)
- [Coding style](docs/engineering/coding-style.md)

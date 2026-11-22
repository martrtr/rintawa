# Rintawa

Rintawa is a local-first runtime for persistent Worlds composed from immutable RTW
packages. Core provides sandboxed execution, exact scoped identities, permissions,
nonblocking extension transport and atomic World storage. Product features and
presentation belong to extensions in rtwKit.

The native template CLI can install immutable recipes, create independent Worlds
and export stopped Worlds with their supported schemas/assets. Browser template
transfer/selection, roleplay onboarding and AI integration are subsequent MVP slices.
See [current delivery and evidence](docs/specs/mvp-story-delivery.md) and
[template contract](docs/specs/world-template-packages.md).

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

Install a template without activating its dependencies, inspect its exact requests,
then explicitly create a new World:

```bash
rintawa package-install ./TextRoleplay.rtw
rintawa template-list
rintawa template-info sha256:<digest>
rintawa world-create --template sha256:<digest> --title "My story" --approve-runtime-permissions
rintawa world-export <world-id> ./saved-story.rtw
```

The separate baseline web-runtime provider must already be selected and locally
permitted. The creation flag approves only the displayed template's World requests;
installation grants nothing and does not start a World. Export requires completed
stop/cleanup, includes all committed World data, and excludes local approvals,
credentials and shell preferences. Installing the resulting RTW and creating from
it makes a new independent World with preserved local graph IDs. No state is merged
into an existing World.

With the matching rtwKit web-runtime/web-ui and installed Text Roleplay package,
the browser's New World dialog selects the template before creation, reviews World
permissions and starts its declared entry. Character/story handles welcome/import
and opens the linked Chat. Core and shell contain no Character-specific dispatch.
AI replies and browser RTW file transfer remain separate unfinished MVP work.

Local acceptance: `scripts/template-cli-smoke.py` uses `RINTAWA_BINARY` and
`RINTAWA_TEMPLATE_ARTIFACTS`. The real-WASM regression requires built rtwKit files:

```bash
RINTAWA_TEMPLATE_ARTIFACTS=/path/to/rtwkit-artifacts \
  cargo test --locked -p rintawa-host --test template_runtime_test -- --ignored
```

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
- no durable jobs, outbox, or World event/signal transport; generic Extension
  Engine event callbacks are in-process only, without World delivery guarantees;
- no actor records or control grants; authority stays Host composition policy;
- World Systems use owner-scoped property reads/writes, including cascade-delete
  checks, while native Host snapshots remain trusted and unrestricted. Property
  ownership is re-established from approved artifacts on each activation, not
  persisted in the transferable four-table World;
- owner-scoped indexed property searches, bounded graph/batch reads and forward/
  reverse relation pagination are exposed to guests without a query language.

## Documentation

- [Architecture](docs/architecture.md)
- [0.0.1 roadmap](docs/roadmap.md)
- [Coding style](docs/engineering/coding-style.md)

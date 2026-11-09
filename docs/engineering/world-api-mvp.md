# Native World API slice

This slice exposes the four-table World read primitives through
`rintawa_world_runtime::WorldSnapshot` and keeps command submission on
`WorldRuntime::submit`.

Available operations are deliberately concrete:

- `read(items, properties)` returns existing requested item records and exact
  selected property records from one pinned SQLite snapshot;
- `find_by_property(key, cursor, limit)` returns an indexed, exclusive-cursor
  page for one exact versioned property key;
- `find_relations(targets, direction, optional_property, cursor, limit)` returns
  relation items adjacent to target objects, with an optional exact property-key
  filter;
- `submit(command)` remains the single-writer, schema-validated command path.

Read batches are limited to 64 item IDs and 64 property keys. Indexed pages are
limited to 256 records and property values to 8 MiB. Missing records are omitted;
there is no query DSL, SQL handle, implicit data repair, or unbounded response.

`WorldSnapshot` owns an SQLite read transaction. Reusing it with a cursor sees a
stable historical snapshot; taking a new snapshot intentionally observes later
commits. Command success still means SQLite has committed the validated change
set.

For command Systems, all requested property keys (including a relation-property
filter) are checked against the active owner binding before SQLite is queried.
The native Host snapshot remains intentionally unrestricted and must not be
passed to an extension.

## Not in this slice

The following remains intentionally absent rather than represented by a stub:

- a WIT/WASM World capability with authenticated component identity;
- Host policy for explicit cross-extension property-read grants, distinct from
  owner-scoped mutation;
- serializable snapshot continuation resources that are revoked on component or
  World stop;
- registration of an extension-defined `WorldSystem` and its one-authoritative
  Zenoh query/reply route.

Those pieces must be introduced together at the Host/Extension Engine boundary:
the Host must derive World and component identity from the active instance, bind
read grants there, revoke resources before lifecycle cleanup, and never accept a
claimed owner, World ID, or principal from guest payloads. A World System bridge
must evaluate outside the Engine/WASM callback lock, avoid network work while a
SQLite write transaction is active, validate the returned transaction, then use
the existing single writer for the atomic commit. The existing generic Zenoh
transport is not treated as this authority boundary by this native slice.

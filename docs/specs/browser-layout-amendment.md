# Browser panel identity and layout persistence

Status: accepted by the owner on 2026-11-22 as a narrow amendment to section 5 of
[mvp-api-and-client-boundaries.md](mvp-api-and-client-boundaries.md).
Acceptance follows the owner's instruction to continue implementation after the
Astra review and its explicit recommendation to accept this amendment. No Core API,
permission, table or transport change is required. Implementation/acceptance tests
remain P10 work.

## Existing behavior and choice

Host CompositionProfile persists ActivationRecord.instance_id, scope_id and artifact;
runtime activation numbers are transient. Existing preferences are scoped to the
executing component and allow a 64 KiB UTF-8 value. The browser contract currently
omits both a persistent panel reference and a layout persistence operation.

Use persisted instance identity, not artifact digest, title, runtime activation or
list position. Digest changes on upgrades; titles and logical package IDs do not
uniquely distinguish instances. A new view registry or fuzzy migration mechanism
is unnecessary: target web-bundle@2 currently has exactly one HTML entry per component.

## Catalog and restoration

Extend each root-shell ui.list descriptor to:

```json
{"id":"opaque-live-id","title":"Chat","worldId":"uuid-or-null","restoreKey":{"worldId":"uuid-or-null","instanceId":"persisted-instance-id","componentId":"manifest-component-id"}}
```

Here uuid-or-null denotes either a canonical UUID string or JSON null, not the
literal string "null". null means Host scope. The provider constructs the key from
its attested hosted context. The sole entry is the view; no separate viewId is
needed until multiple independently addressable views per component are required.
Identifiers are opaque strings and compared exactly, never concatenated ambiguously.

ui.mount remains {id}: resolve a saved restoreKey against the CURRENT ui.list and
mount only its unique live opaque id. A stored key grants no authority. The server
rechecks liveness and session/channel ownership at mount. A stale list/mount race
returns channel-closed or unavailable; refresh the catalog, never reuse old channels.

Persisted instance/component/world identity survives restart. Upgrade restores only
if the approved composition retains that instanceId and the package retains that
componentId with the same web target. Reinstall/new selection/new instance ID, renamed
component, missing/disabled/inactive World or duplicate catalog key yields a removable
placeholder. No title/package/digest fallback, automatic World activation or install.
An update with changed identity needs an explicit user selection; no migration API.

Multiple DockView panels may reference one restoreKey. Each has a distinct shell-local
panelId and separately mounted channel/frame; duplicates do not create extension
activations. A restored panelId is UI state only. Limit mounted channels as already
specified (32 total). On World switch, close previous channels and resolve keys for
the selected World afresh; never substitute the current World into a saved key.

## Shell-only persistence wire operations

Add only these root-shell methods to the allowlist:

- ui.layout.get({}) -> layout object or null.
- ui.layout.set({layout}) -> null after successful preference persistence.
- ui.layout.reset({}) -> null after successful deletion.

Execute existing preferences.get/set/delete under the selected shell's dependent
context, with the fixed key rintawa.ui.layout.v1. Never use the web-runtime provider's
own preferences. The caller cannot supply an owner, scope, preference key or path.
Panels and unselected root components are denied. No general preferences dispatcher.

Layout has {version:1, panels:[{panelId,restoreKey}], dockview:object}; panelId is a
nonempty ASCII string of at most 64 characters and unique in panels. At most 32 panels,
JSON nesting at most 32, and serialized UTF-8 layout at most 64 KiB. Validate root and
panel fields, keys and types; the shell validates DockView data against its pinned
version before use, including references to panelIds. Treat it as inert state, never
HTML, URLs or dispatch instructions. This first layout contract persists no arbitrary
view parameters. Domain view-state contracts can be designed separately when needed.

The serialized preference contains no tokens, channels, handles or activation numbers.
Malformed/unsupported stored data falls back to the initial workspace without executing
or automatically overwriting it. Missing views retain their saved keys as placeholders.
Save is debounced and reports errors; successful replies follow persistence. Existing
frame and preference limits apply. Multiple authenticated shell sessions share this
shell preference: whole-layout last successful write wins, no implicit merge or CAS.
Replacing the shell instance does not transfer its preferences automatically.

## Acceptance for P10

1. Restart with new tokens/activation/opaque ids restores the same persisted instances.
2. Two Worlds, repeated panels, and distinct instances never cross-bind; missing,
   disabled, upgraded-with-new-ID and ambiguous entries produce placeholders.
3. Catalog changes between list and mount cannot reuse an obsolete activation.
4. Shell save/get/reset survives Host restart; panel attempts and forged owner/key
   fields fail before dispatch; saving with a different provider does not change owner.
5. Invalid JSON shapes, excessive depth/size, unknown versions and invalid DockView
   references safely fall back; failed saves remain visible to the user.
6. Multiple sessions exhibit documented last-write behavior. Channel close/remount
   releases transient resources and never persists credentials.

This is sufficient for layout restoration, not a generic multi-view schema or a
promise of compatibility across future DockView serialization changes.

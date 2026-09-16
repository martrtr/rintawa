# Rintawa

Rintawa Core hosts exact local RTW artifacts. Network repositories, version discovery,
updates, and dependency solving belong to replaceable Package Manager extensions.

Current clean-build bootstrap:

```bash
rintawa install ./package.rtw
rintawa list
rintawa disable <subject>
rintawa enable <subject>
rintawa run
```

Artifacts are validated and stored immutably by SHA-256. `profiles/baseline.toml`
contains the pre-world activation composition required to start UI and host tools.
Future State Engine worlds are expected to add their own overlays over the same CAS
instead of turning the bootstrap/main-menu scope into a world.

The RTW root manifest intentionally stays minimal: container format, versioned content
type, and content-specific entry descriptor. Content handlers own richer metadata.

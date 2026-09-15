# rintawa-dev

Development CLI for Rintawa extensions.

```bash
cargo run -p rintawa-dev -- check path/to/extension
```

Projects that require a build step can add `rintawa-dev.toml`:

```toml
schema = 1
artifact-root = "build/rtw"
build = ["npm", "run", "build"]
```

`check` builds the project, creates a temporary RTW artifact, and validates it through the artifact store.

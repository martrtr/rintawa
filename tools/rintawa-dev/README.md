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

Run the same snapshot through the Extension Engine:

```bash
cargo run -p rintawa-dev -- run path/to/extension
```

Use `--once` to start and stop immediately for lifecycle checks.

Watch and reload after stable source changes:

```bash
cargo run -p rintawa-dev -- watch path/to/extension
```

`watch-ignore` uses gitignore-style patterns:

```toml
watch-ignore = ["*.tmp", "**/build/", "packages/**/node_modules/"]
```

`.git`, `target`, and `node_modules` are ignored by default. A separate build
`artifact-root` is ignored automatically while a build command is configured.

# Contributing to Taverna

Run the complete local verification suite before opening a pull request:

```bash
bash scripts/check.sh
```

The same command runs in CI: formatting, Clippy for all targets and features,
tests, and documentation generation.

To enable the versioned pre-commit hook for this clone, run once:

```bash
git config core.hooksPath .githooks
```

The SDK follows [the Rust coding style](docs/engineering/coding-style.md). Its
public API is intentionally small; add a new public contract only after it is
needed by a working engine pipeline.

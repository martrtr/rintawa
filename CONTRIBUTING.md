# Contributing to Rintawa

## Development environment

Rust 1.98.1 is the canonical development toolchain. The repository provides a
reproducible Nix development shell, including `cargo`, `clippy`, and `rustfmt`:

```bash
nix develop
```

With direnv installed, run `direnv allow` once at the repository root to enter
the same shell automatically. Developers using rustup can rely on
`rust-toolchain.toml`, which specifies the identical toolchain.

Run the complete local verification suite before opening a pull request:

```bash
nix develop --command bash scripts/check.sh
```

When already inside `nix develop`, the shorter `bash scripts/check.sh` is
equivalent. The same checks run in CI: formatting, Clippy for all targets and
features, tests, and documentation generation.

To enable the versioned pre-commit hook for this clone, run once:

```bash
git config core.hooksPath .githooks
```

The hook invokes the Nix shell automatically. When Nix is unavailable, it uses
the Rust 1.98.1 toolchain selected by rustup instead.

The SDK follows [the Rust coding style](docs/engineering/coding-style.md). Its
public API is intentionally small; add a new public contract only after it is
needed by a working engine pipeline.

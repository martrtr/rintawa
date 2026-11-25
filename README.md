<p align="center">
  <img alt="Rintawa banner" src="https://github.com/user-attachments/assets/541ba9ce-2678-4ce4-82ec-5881cd0bfc4a" />
</p>

<div align="center">

**[Architecture](docs/architecture.md)** · **[Roadmap](docs/roadmap.md)** · **[Contributing](CONTRIBUTING.md)** · **[rtwKit](https://github.com/martrtr/rtwKit)** · **[Discord](https://discord.gg/gAdfJZySPD)** · **[Fluxer](https://fluxer.gg/DK0198jA)**

[![Status](https://img.shields.io/badge/status-early%20development-F59E0B)](docs/roadmap.md)
[![Rust](https://img.shields.io/badge/Rust-1.98.1-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPL--3.0--only-2563eb)](LICENSE)

</div>

# Rintawa

Rintawa is a local-first runtime for persistent Worlds assembled from immutable
RTW packages. Core owns package execution, owner-scoped permissions, bounded
extension transport and atomic World storage; product behavior and presentation
belong to extension packages.

The runtime provides:

- **Persistent Worlds:** four-table SQLite state, bounded single-writer commands
  and atomic change sets.
- **Composable extensions:** sandboxed WASM execution, exact scoped identities,
  lifecycle cleanup and explicit capabilities.
- **RTW packages:** content-addressed installation, exact activation and portable
  World templates.
- **Extension transport:** bounded request/reply and notification primitives owned
  by the active components rather than a Core UI framework.

The standard web interface and application packages are developed in
[rtwKit](https://github.com/martrtr/rtwKit).

## Status

Rintawa is in early development. The `dev` branch contains the current MVP Core;
the end-to-end character, Chat and AI experience is supplied by compatible
[rtwKit](https://github.com/martrtr/rtwKit) packages. The full `0.0.1` experience
has not been released.
See the [roadmap](docs/roadmap.md) and [open issues](https://github.com/martrtr/rintawa/issues)
for current progress.

## Build

Rust 1.98.1 is specified in `rust-toolchain.toml`. A Nix development shell is also
available.

```sh
git clone https://github.com/martrtr/rintawa.git
cd rintawa
git switch dev
cargo build --locked --release -p rintawa
cargo test --locked --workspace
```

The CLI installs exact RTW artifacts, creates isolated Worlds and starts a
configured composition:

```sh
./target/release/rintawa install ./package.rtw
./target/release/rintawa list
./target/release/rintawa package-install ./TextRoleplay.rtw
./target/release/rintawa template-list
./target/release/rintawa world-create --template sha256:<digest> --approve-runtime-permissions
./target/release/rintawa run
```

A fresh build does not include a preconfigured graphical experience. The compatible
web runtime, UI and application packages are maintained separately in rtwKit. See
[the architecture](docs/architecture.md) for lifecycle and capability boundaries,
and [the roadmap](docs/roadmap.md) for the remaining MVP work.

For development checks and contribution guidelines, see
[CONTRIBUTING.md](CONTRIBUTING.md).

## Star History

<a href="https://www.star-history.com/?repos=martrtr%2Frintawa&type=date&legend=bottom-right">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=martrtr/rintawa&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=martrtr/rintawa&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=martrtr/rintawa&type=date&legend=top-left" />
 </picture>
</a>

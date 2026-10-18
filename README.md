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

Rintawa is a local-first runtime for persistent interactive worlds. It manages
world state, extension execution, permissions and presentation while keeping
experiences configurable through RTW packages.

The runtime provides:

- **Persistent worlds:** authoritative state changes, events and SQLite-backed recovery.
- **Composable extensions:** versioned contracts, scoped services and isolated execution.
- **RTW packages:** content-addressed artifacts with explicit installation and activation.
- **Portable UI:** extension-provided interfaces rendered by a selected UI layer.

The standard web interface and application packages are developed in
[rtwKit](https://github.com/martrtr/rtwKit).

## Status

Rintawa is in early development. The engine and initial World/Chat workflows are
available on the `dev` branch; the full `0.0.1` experience has not been released.
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

The CLI manages installed RTW artifacts and starts a configured composition:

```sh
./target/release/rintawa install ./package.rtw
./target/release/rintawa list
./target/release/rintawa run
```

A fresh build does not include a preconfigured graphical experience. The compatible
web runtime, UI and application packages are maintained separately in rtwKit.

For development checks and contribution guidelines, see
[CONTRIBUTING.md](CONTRIBUTING.md).

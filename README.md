<div align="center">

# Taverna

  ### Extensible engine for interactive narrative worlds

Build stateful worlds powered by **systems, extensions and AI**.

[![Status](https://img.shields.io/badge/status-early%20development-f59e0b)](#status)
[![Rust](https://img.shields.io/badge/Rust-1.88-000000?logo=rust\&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPL--3.0--only-2563eb)](./LICENSE)
[![Discord](https://img.shields.io/badge/Discord-Join-5865F2?logo=discord\&logoColor=white)](https://discord.gg/gAdfJZySPD)

**[Documentation](./docs)** · **[Discord](https://discord.gg/gAdfJZySPD)** · **[Development](../../tree/dev)**

</div>

<img width="2160" height="1080" alt="image" src="https://github.com/user-attachments/assets/29a4c02c-6c54-4ae9-9af6-6286a4bdcd10" />

## About

**Taverna** is an extensible engine for creating interactive, stateful and AI-driven worlds.

Instead of hard-coding concepts such as characters, inventories, quests or dialogue into the engine, Taverna provides a small set of fundamental primitives and an extension system that lets projects define their own mechanics, systems and behavior.

## Status

> [!WARNING]
> Taverna is in **early development**. APIs and architecture may change significantly.

The project is currently focused on the SDK, Extension Engine and the first executable extensions.

For technical details and architecture, see [`docs/`](./docs).

## Development

Taverna currently targets **Rust 1.88** and **Edition 2024**.

```bash
git clone https://github.com/martrtr/taverna.git
cd taverna
git switch dev

cargo check --workspace
cargo test --workspace
```

See [`docs/engineering/coding-style.md`](./docs/engineering/coding-style.md) before contributing.

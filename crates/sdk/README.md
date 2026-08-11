# Taverna SDK

`taverna-sdk` defines the public Rust contracts for Taverna extensions.

Version 0.0.1 deliberately establishes only the portable foundation:

- typed extension, component, contribution, and target identifiers;
- serializable manifest and contribution descriptors;
- component lifecycle and registration traits;
- a logging API and common error type.

The SDK does not contain an Extension Engine, Wasmtime, React, state/storage/AI
APIs, permission enforcement, dependency resolution, or a System scheduler.
Those are host or engine responsibilities and will be introduced only when a
real end-to-end pipeline needs them.

## Runtime targets

A component's `kind` defines its role, while its `target` selects the host
contract. A runtime component initially runs through the native host:

```toml
id = "chat"
name = "Chat"
version = "0.0.1"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "native"
```

The same component can later use a WIT-based adapter by changing only its
target and entry:

```toml
target = "wasm"
entry = "runtime.wasm"
```

WIT will be the WASM ABI. The Rust [`Component`] trait remains the ergonomic
author-facing interface; Wasmtime belongs to the Extension Engine, never to
this SDK.

React is likewise not part of this crate. It is an implementation detail of
the official Web UI Host. A public UI component uses `kind = "ui"` and a
host-specific target such as `"web"`; Godot or another application can provide
another compatible UI host without changing runtime components.

## Lifecycle

The Extension Engine owns creation, ordering, rollback, and cleanup. A
component only registers its capabilities and participates in the lifecycle:

```rust
use taverna_sdk::prelude::*;

pub struct ChatRuntime {
    id: ComponentId,
}

impl ChatRuntime {
    pub fn new() -> Self {
        Self {
            id: ComponentId::from("runtime"),
        }
    }
}

impl Component for ChatRuntime {
    fn id(&self) -> &ComponentId {
        &self.id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        ctx.register(ContributionDescriptor::new(
            "chat.system",
            ContributionKind::system(),
        ))?;
        Ok(())
    }
}
```

The first engine milestone is intentionally small: parse this manifest,
register a component, observe its registered contributions, start it, and
remove those contributions after `stop` during deactivation.

[`Component`]: https://docs.rs/taverna-sdk/latest/taverna_sdk/traits/trait.Component.html

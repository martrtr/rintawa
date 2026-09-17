# Rintawa SDK

`rintawa-sdk` defines the public Rust contracts for Rintawa extensions.

Version 0.0.1 deliberately establishes only the portable foundation:

- typed extension, component, contribution, and target identifiers;
- serializable manifest and contribution descriptors;
- component lifecycle and registration traits;
- a logging API and common error type.

The SDK does not contain an Extension Engine, Wasmtime, React, state/storage/AI
APIs, general permission enforcement, dependency resolution, or a System
scheduler. Secret-read requests are an exception: the SDK can declare them,
but only the Rintawa host can grant and serve them.
Those are host or engine responsibilities and will be introduced only when a
real end-to-end pipeline needs them.

## Runtime targets

A component's `target` answers only **how that component is executed**. Product
roles and composition semantics are expressed separately through versioned
contracts. Core permanently provides the root WASM execution target:

```toml
id = "chat"
name = "Chat"
version = "0.0.1"
sdk = "^0.0"

[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "runtime.wasm"
```

WIT is the built-in WASM ABI. The Rust [`Component`] trait remains the ergonomic
author-facing interface; Wasmtime belongs to the Extension Engine, never to this
SDK. Additional execution targets are not hard-coded product features: the
bootstrap architecture is designed so extensions can provide new versioned
targets.

Presentation technology is likewise outside this SDK. A shell, UI layer, TUI,
3D frontend, or Web frontend should advertise the role it provides through
versioned contracts independently of its execution target. For example, the
platform-owned `rintawa.host.shell@1` binding selects the primary Host Shell; it
does not imply Web, React, an Android Activity, or any specific runtime.

## Lifecycle

The Extension Engine owns creation, ordering, rollback, and cleanup. A
component only registers its capabilities and participates in the lifecycle:

```rust
use rintawa_sdk::prelude::*;

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

[`Component`]: https://docs.rs/rintawa-sdk/latest/rintawa_sdk/traits/trait.Component.html

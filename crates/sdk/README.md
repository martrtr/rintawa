# Rintawa SDK

`rintawa-sdk` defines the public Rust contracts for Rintawa extensions.

Version 0.0.1 deliberately establishes only the portable foundation:

- typed extension, component, contribution, and target identifiers;
- serializable manifest and contribution descriptors;
- component lifecycle and registration traits;
- a logging API and common error type.

The SDK does not contain an Extension Engine, Wasmtime, React, state/storage/AI
APIs, permission enforcement, dependency resolution, or a System scheduler. It
only defines capability requests in public data contracts: secret-read patterns
and coarse runtime permissions. A request never grants access by itself; only the
Rintawa host can approve a concrete component principal and serve the capability.
Enforcement, resource ownership, scheduling, and network implementation remain
host or engine responsibilities.

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

## Runtime capability requests

A component may request coarse host capabilities in its manifest:

```toml
[[components]]
id = "runtime"
kind = "runtime"
target = "rintawa.runtime.wasm-component@1"
entry = "runtime.wasm"

[components.permissions]
runtime = ["background-task", "loopback-listen", "loopback-connect"]
```

These values are requests, not grants. Host policy approves an exact component
principal separately. `background-task` maps to cooperative host-pumped work;
it is not a guest thread. The current network boundary is deliberately narrower
than general network access: `loopback-listen` and `loopback-connect` expose
bounded host-owned TCP handles restricted to the local loopback interface. They
do not grant DNS, arbitrary outbound sockets, filesystem access, or raw OS file
descriptors.

Execution through another runtime provider does not inherit that provider's
runtime permissions. Delegated components execute under their own principal.

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

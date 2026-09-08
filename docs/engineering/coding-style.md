# Coding Style

This document defines the coding standards for the Rintawa project. It applies to all Rust code in the codebase.

> **Note**: These rules are enforced by CI. Use `cargo fmt` and `cargo clippy` before committing.

---

## Formatting

We use the default Rustfmt configuration. No custom `rustfmt.toml` is required.

```bash
cargo fmt --all
```

**Rules:**
- Maximum line width: 100 characters (Rustfmt default)
- Indentation: 4 spaces (no tabs)
- Use `rustfmt` — no manual formatting debates

---

## Naming

| Concept | Style | Example |
|---------|-------|---------|
| Types (structs, enums, traits) | `UpperCamelCase` | `StateEngine`, `Extension` |
| Functions, methods, variables | `snake_case` | `register_service()`, `project_path` |
| Constants | `SCREAMING_SNAKE_CASE` | `MAX_RETRIES`, `DEFAULT_TIMEOUT` |
| Type parameters | `UpperCamelCase` (single letter or descriptive) | `T`, `Provider` |
| Modules and crates | `snake_case` | `extension_engine`, `rintawa_core` |
| Lifetimes | `'lowercase` (single letter preferred) | `'a`, `'ctx` |
| Enum variants | `UpperCamelCase` | `Loaded`, `Unloaded` |
| Feature flags | `snake_case` | `wasm_support` |

**Additional rules:**
- Use **full words** in names (avoid abbreviations unless universally known: `id`, `ctx`).
- **Boolean** variables/return values should start with `is_`, `has_`, `can_`, `should_`.
- **Getters** are named like the field: `.name()` not `.get_name()`.
- **Setters** are named `set_<field>()`.

---

## Modules

### Structure

```
core/
├── state/              # module
│   ├── mod.rs          # public exports
│   ├── engine.rs       # StateEngine implementation
│   ├── events.rs       # Event types
│   └── transactions.rs
├── storage/
│   ├── mod.rs
│   ├── sqlite.rs
│   └── migration.rs
└── extension/
    ├── mod.rs
    ├── registry.rs
    └── lifecycle.rs
```

### Module Discipline

- Each module has `mod.rs` that exports only the **public API**.
- **Private** implementation details stay in submodules.
- Use `pub(crate)` for visibility within the crate (not public API).
- **Avoid deeply nested modules** (>3 levels) unless necessary.

### Imports

```rust
// Standard library first
use std::collections::HashMap;
use std::sync::Arc;

// External crates
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Internal crates
use crate::core::state::{State, StateEvent};

// Separate groups with blank lines
```

- Prefer `use` over fully qualified paths in function bodies.
- Use `crate::` for internal imports (not `super::` unless in tests).

---

## Traits

### Design Principles

- **One trait per coherent responsibility** (single responsibility).
- Prefer **small, focused traits** over large, monolithic ones.
- Provide **default implementations** where sensible.

### Naming Convention

- Traits are nouns or adjectives: `Extension`, `Provider`, `Serializable`.
- If a trait represents an action, name it as a verb: `Render`, `Validate`.

### Example

```rust
/// A service that can be registered in the container.
pub trait Service: Send + Sync {
    fn name() -> &'static str;
}

/// Extensions implement this to be loaded into the runtime.
pub trait Extension: Send + Sync {
    fn activate(&self, ctx: ExtensionContext) -> Result<(), ExtensionError>;
    fn deactivate(&self) -> Result<(), ExtensionError> {
        Ok(())  // default: no-op
    }
}
```

### Associated Types vs Generics

- Use **associated types** when the trait has exactly one logical output type.
- Use **generics** when the trait should be implemented multiple times for different types.

```rust
// ✅ Associated type
pub trait Provider {
    type Output;
    fn provide(&self) -> Self::Output;
}

// ✅ Generic (can be implemented for multiple types)
pub trait Process<T> {
    fn process(&self, input: T) -> Result<T, ProcessError>;
}
```

---

## Errors

### Error Handling Strategy

- Use **`Result<T, E>`** for fallible operations.
- Use **`Option<T>`** for optional values (not errors).
- **Don't use `unwrap()` or `expect()`** in production code. Use `?` and handle errors at boundaries.
- Use `anyhow` for application-level errors (binaries, tests).
- Use `thiserror` for library-level errors (crates).

### Defining Error Types

```rust
use thiserror::Error;

#[derive(Error, Debug, Clone)]
pub enum StateError {
    #[error("object '{id}' not found")]
    ObjectNotFound { id: String },

    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("storage error: {source}")]
    Storage {
        #[from]
        source: StorageError,
    },
}
```

### Error Hierarchy

- Each module has its own error type (e.g., `StateError`, `StorageError`).
- Use `#[from]` for error chaining.
- **Don't expose implementation details** in error messages (keep them user-friendly).
- Use `Box<dyn Error>` only in tests or application boundaries.

---

## Documentation

### Comments

| Type | Syntax | When |
|------|--------|------|
| **Module doc** | `//!` | At top of `mod.rs` or file |
| **Item doc** | `///` | Before structs, functions, traits, enums |
| **Internal doc** | `//` | Explanations inside implementation |
| **TODO/FIXME** | `// TODO:` or `// FIXME:` | Track pending work |

### Required Documentation

Every **public item** must have a doc comment:

```rust
/// Represents a single object in the world.
///
/// Objects are the minimal units of existence in the Rintawa world model.
/// They have an ID, a type, and arbitrary properties.
///
/// # Example
///
/// ```
/// let obj = Object::new("alice", "character")
///     .with_property("age", 24);
/// ```
pub struct Object {
    // ...
}
```

**What to document:**
- Purpose of the item
- When to use it (and when not)
- Panics (if any)
- Examples (for complex APIs)

### Markdown in Docs

- Use `# Examples` sections for code examples.
- Use `# Panics` if the function can panic.
- Use `# Errors` to describe error cases.

```rust
/// Loads a project from the given path.
///
/// # Errors
///
/// Returns `StorageError::NotFound` if the path doesn't exist.
/// Returns `StorageError::InvalidData` if the project file is malformed.
pub fn load_project(path: &Path) -> Result<Project, StorageError> {
    // ...
}
```

---

## Testing

### Test Structure

```
src/
  state/
    engine.rs
    engine_tests.rs   # adjacent test module
```

or

```
src/
  state/
    engine.rs
tests/
  state_tests.rs      # integration tests
```

### Unit Tests

Place unit tests in a `test` module within the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_creation() {
        let obj = Object::new("alice", "character");
        assert_eq!(obj.id(), "alice");
    }
}
```

### Integration Tests

Place integration tests in `tests/` directory at crate root:

```rust
// tests/state_integration.rs
use rintawa_core::state::StateEngine;

#[test]
fn test_state_transaction() {
    let engine = StateEngine::new();
    // ... test full workflow
}
```

### Test Naming

- Tests are named `test_<what_is_tested>_<condition>`.
- Use `should` for expected behavior: `test_should_return_error_on_invalid_id`.

### Test Utilities

- Use `assert_eq!`, `assert!`, `assert_matches!` (from `assert_matches` crate).
- Prefer `anyhow::Result` in tests for simplicity.
- Use `#[should_panic]` only when testing panic conditions.

---

## Code Organization Checklist

Before committing:

- [ ] `cargo fmt --all` has been run
- [ ] `cargo clippy` passes with no warnings
- [ ] All public items have doc comments
- [ ] No `unwrap()` or `expect()` in production code
- [ ] Tests are in the correct location
- [ ] Imports are grouped and sorted

---

## Tools

| Tool | Command | Purpose |
|------|---------|---------|
| `rustfmt` | `cargo fmt` | Auto-format code |
| `clippy` | `cargo clippy` | Linting |
| `cargo test` | `cargo test` | Run tests |
| `cargo doc` | `cargo doc --open` | Generate and open docs |
| `cargo check` | `cargo check` | Fast compile checking |

---

## Exceptions

Exceptions to any rule must be:
1. **Documented** with a comment explaining why.
2. **Approved** in code review (for obvious exceptions, no approval needed).

```rust
// SAFETY: This is safe because we hold the lock and the pointer is valid.
unsafe { ptr.read() }
```
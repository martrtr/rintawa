# Coding style

These conventions apply to Rust contributions to Rintawa. The repository's
formatting, linting, tests, documentation checks and secret scanning are run
through `scripts/check.sh`. Follow the existing implementation when a local
convention is more specific.

## Formatting and names

- Use the default `rustfmt` configuration (100-column width, four-space indentation).
- Use `UpperCamelCase` for types and traits, `snake_case` for functions,
  modules and variables, and `SCREAMING_SNAKE_CASE` for constants.
- Prefer descriptive names. Boolean predicates use `is_`, `has_`, `can_` or
  `should_` where appropriate.
- Name getters after the value (`name()`) and mutating setters `set_name()`.
- Keep imports grouped: standard library, external crates, then local modules.
- Use `crate::` paths for internal imports. Prefer narrow public exports and
  `pub(crate)` for crate-internal APIs.

## API and error handling

- Give each trait one coherent responsibility. Use associated types when one
  output type belongs to the implementation; use generics for multiple inputs.
- Return `Result<T, E>` for fallible operations and `Option<T>` for absence.
- Use typed errors (`thiserror`) in libraries and `anyhow` at application or
  test boundaries where appropriate.
- Avoid `unwrap()` and `expect()` in production paths. Handle errors at the
  relevant boundary and preserve useful context.
- Document public APIs, meaningful invariants and error conditions. Use
  `# Errors` and `# Panics` sections where applicable.

## Modules and tests

- Keep module boundaries focused and implementation details private.
- Place unit tests beside the implementation, inside `#[cfg(test)]` modules;
  use crate-level `tests/` for integration tests.
- Name tests after the behavior and condition under test.
- Cover permission boundaries, failure paths and state transitions alongside
  successful behavior.

## Secrets

Never commit credentials, access tokens or private keys. Keep local credentials
in ignored configuration or the Host's secret capability.

`scripts/check.sh` includes a pinned secret scan. If a real credential reaches
Git history, revoke or rotate it; removing it from the working tree is not enough.
Scanner exceptions are reserved for verified false positives.

## Before opening a pull request

```sh
bash scripts/check.sh
```

This runs the repository's verification workflow. Review the staged diff for
unrelated files, unsafe error handling, unnecessary API changes and accidental
exposure of private configuration before committing.

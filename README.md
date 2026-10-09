# Rintawa

Rintawa is a local-first runtime for persistent Worlds composed from immutable RTW
packages. Core provides generic package execution, scoped composition, authoritative World
state and renderer-neutral UI infrastructure; product features are extensions.

Standard packages and the Web experience live in
[rtwKit](https://github.com/martrtr/rtwKit).

## Quick start

```bash
rintawa install ./package.rtw
rintawa list
rintawa disable <subject>
rintawa enable <subject>
rintawa run
```

RTW artifacts are validated and stored immutably by SHA-256. Global tools run in the
baseline composition; new Worlds are materialized from an independent default-world recipe
and then keep their own exact composition.

## Documentation

- [Architecture](docs/architecture.md)
- [0.0.1 roadmap](docs/roadmap.md)
- [Coding style](docs/engineering/coding-style.md)

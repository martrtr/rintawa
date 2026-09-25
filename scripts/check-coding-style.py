#!/usr/bin/env python3
"""Enforce mechanically checkable Rust rules from docs/engineering/coding-style.md."""

from __future__ import annotations

from pathlib import Path
import sys

ROOTS = (Path("crates"), Path("apps"), Path("tools"))
SKIPPED_PARTS = {"target", "generated", "node_modules"}


def rust_files() -> list[Path]:
    files: list[Path] = []
    for root in ROOTS:
        if not root.exists():
            continue
        for path in root.rglob("*.rs"):
            if any(part in SKIPPED_PARTS for part in path.parts):
                continue
            files.append(path)
    return sorted(files)


def first_substantive_line(path: Path) -> str | None:
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#!["):
            continue
        return line
    return None


def is_test_file(path: Path) -> bool:
    return (
        "tests" in path.parts
        or path.name in {"tests.rs", "test.rs"}
        or path.stem.endswith("_test")
        or path.stem.endswith("_tests")
    )


def main() -> int:
    errors: list[str] = []
    for path in rust_files():
        first_line = first_substantive_line(path)
        if first_line is None or not first_line.startswith("//!"):
            errors.append(f"{path}: Rust modules must start with a //! module doc comment")

        if is_test_file(path):
            continue
        for number, raw_line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            # Rustfmt indents imports inside nested modules. Only column-zero imports are
            # module-scope production imports; `use super::*` inside cfg(test) modules is allowed.
            if raw_line.startswith("use super::") or raw_line.startswith("pub use super::"):
                errors.append(
                    f"{path}:{number}: production imports must use crate:: instead of super::"
                )

    if errors:
        print("coding-style check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1
    print("coding-style check passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

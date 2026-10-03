# Decisions

Significant decisions and the reasons for them. Every added dependency is recorded here.

## Dependencies

None yet.

## Decisions

### 2026-10-04: Scaffold only the Rust crates for M1

`src-tauri/` and `ui/` are left out until M2, since the Tauri app is out of scope for M1. This keeps the build and CI small.

### 2026-10-04: Enforce "no unwrap or expect" with clippy

The workspace denies `clippy::unwrap_used` and `clippy::expect_used`, and `clippy.toml` allows them in tests. The rule from `CLAUDE.md` is checked by the linter instead of by review.

### 2026-10-04: Forbid unsafe code

The workspace sets `unsafe_code = "forbid"`. Nothing in this app needs it.

### 2026-10-04: LF line endings in the repo

`.gitattributes` normalises text files to LF, so checkouts look the same on every machine and CI formatting checks are stable.

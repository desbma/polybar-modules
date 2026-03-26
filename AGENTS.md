# AGENTS.md

## Build & Check Commands

- Build: `cargo build`
- Check/Lint: `cargo clippy --all-targets` (pedantic + restriction lints enabled)
- Format: `cargo +nightly fmt --check -- --config imports_granularity=Crate --config group_imports=StdExternalCrate`
- Test: `cargo test`
- Single test: `cargo test <test_name>`

## Architecture

Single-binary Rust CLI that outputs status lines for Polybar. Each module in `src/polybar_module/` implements the `RenderablePolybarModule` trait with `wait_update`, `update`, and `render` methods. Modules are registered in `mod.rs` and selected via CLI (clap). Uses Rust Edition 2024 with nightly toolchain.

## Code Style

- No comments unless complex; docs required for public items (`missing_docs = "warn"`)
- Strict Clippy: pedantic + many restriction lints (see `[lints.clippy]` in Cargo.toml)
- Errors: use `anyhow::Result` for fallible functions; avoid `expect`/`panic` outside tests
- Imports:
  - Group std imports first, then external crates, then local modules
  - Never use fully-qualified paths (e.g., `std::path::Path` or `crate::ui::foo()`) in code; always import namespaces via `use` statements and refer to symbols by their short name
  - Import deep `std` namespaces aggressively (e.g., `use std::path::PathBuf;`, `use std::collections::HashMap;`), except for namespaces like `io` or `fs` whose symbols have very common names that may collide — import those at the module level instead (e.g., `use std::fs;`)
  - For third-party crates, prefer importing at the crate or module level (e.g., `use anyhow::Context as _;`, `use clap::Parser;`) rather than deeply importing individual symbols, to keep the origin of symbols clear when reading code — only import deeper when needed to avoid very long fully-qualified namespaces
  - Never use inline imports (e.g., `use` inside a function body or block), unless the code block is gated by conditional compilation (e.g., `#[cfg(test)]`)
- Prefer `log` macros for logging; no `dbg!` or `todo!`
- Prefer `default-features = false` for dependencies
- Do not add `derive` traits unless they are required by the current code (compile errors) or actively used by tests/runtime behavior
- In tests:
  - Use `use super::*;` to import from the parent module
  - Prefer `unwrap()` over `expect()` for conciseness
  - Do not add custom messages to `assert!`/`assert_eq!`/`assert_ne!` — the test name is sufficient
  - Prefer full type comparisons with `assert_eq!` over selectively checking nested attributes or unpacking; tag types with `#[cfg_attr(test, derive(Eq, PartialEq))]` if needed
- When moving or refactoring code, never remove comment lines — preserve all comments and move them along with the code they document

## Version control

- This repository uses the jujutsu VCS. **Never use any `jj` command that modifies the repository**.
- You can also use read-only git commands for inspecting repository state. **Never use any git command that modifies the repository**.

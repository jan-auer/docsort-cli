# docsort2

A single-crate Rust CLI application.

## Collaboration Model

All implementation work is delegated to agents. Design decisions are surfaced to the user. Agents must ask clarifying questions rather than making architectural assumptions.

When spawning agents, choose the model appropriate to the task:
- **Haiku**: simple, mechanical tasks (file ops, formatting, single-step commands)
- **Sonnet**: standard implementation, refactoring, moderate reasoning
- **Opus**: complex architecture, deep reasoning, critical decisions

Spawn one agent per independent concern — keep each agent's context small and focused on a surgical change. Run agents serially (one after another), not in parallel — concurrent agents risk conflicting edits and git conflicts. When a batch of changes contains independent concerns (e.g. Ctrl+C behaviour vs. rendering vs. layout), split them into separate agents rather than bundling into one.

Each independent change gets its own commit. If an agent implements multiple independent changes, it must commit them separately.

## Toolchain

- Rust: latest stable
- Formatter: `rustfmt` with default config (no `rustfmt.toml` overrides)
- Linter: `cargo clippy -- -D warnings`
- No MSRV constraint

## Check Commands

Run before declaring work done:

```
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

Hooks run these automatically: `rustfmt` fires on every `.rs` file edit; `clippy` and `cargo test` fire at the end of each agent cycle.

## Error Handling

- Use `anyhow` for most error propagation in CLI code
- Use `thiserror` only when callers need to inspect error variants
- No `.unwrap()` outside of tests

## Testing

- Unit tests: inline in each module (`#[cfg(test)]`)
- Integration tests: `tests/` directory
- Keep tests minimal — this CLI delegates heavy lifting to libraries
- Write a failing test before implementing new functionality or fixing bugs

## Dependencies

Check for existing equivalents before adding a new dependency.

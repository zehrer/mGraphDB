# mGraphDB

A Rust project scaffold.

## Development
- Build: `cargo build`
- Run: `cargo run`
- Test: `cargo test`

## Commit Message Convention
This repo uses Conventional Commits and enforces them via a Git `commit-msg` hook.

- Format: `<type>(<scope>)!: <subject>`
- Common types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`
- `!` indicates a breaking change; alternatively add a `BREAKING CHANGE:` footer.

A commit message template is configured at `.gitmessage.txt`. To use it interactively, run `git commit` without `-m` and your editor will open the template.

## Hooks Setup
Hooks are stored under `.githooks` and Git is configured to use this path.
If hooks ever fail to run, ensure the scripts are executable: `chmod +x .githooks/*`.
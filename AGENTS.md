# Repository workflow

- Use Git actively. Inspect `git status`, `git diff`, and `git diff --cached` before committing. Stage changes by path or hunk and preserve existing user changes. Commit each validated unit of work with a descriptive message.
- Enable the repository's pre-commit hook with `git config --local core.hooksPath .githooks`. Git runs `cargo fmt --all` before committing and requires `cargo clippy --workspace --all-targets --all-features` to succeed.
- The hook stages formatting for Rust files already fully staged. Resolve partial staging of a Rust file before committing so its unstaged edits stay out of the commit.

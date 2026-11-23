# Contributing to Rust Background Muter

Thank you for your interest in contributing. This guide gets you set up and tells you what the checks cost and what they do not cover.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you agree to follow it. Please report unacceptable behavior by opening an issue at https://github.com/cosmokud/rust-bg-muter/issues.

## Ways to contribute

You don't need to write code to help. Useful contributions include:

- Reporting bugs (something doesn't work as expected).
- Suggesting new features or improvements.
- Improving documentation (README, comments, guides).
- Fixing bugs or implementing features.
- Testing builds and reporting what you find.

## Before you start

1. Check the [open issues](https://github.com/cosmokud/rust-bg-muter/issues) to see if your bug or idea is already reported. If it is, add a comment instead of opening a duplicate.
2. For a new feature or a large change, open an issue first and describe your idea. This avoids duplicated work and lets maintainers give feedback early.
3. If you want to work on an existing issue, leave a comment saying so.

## Getting set up

Requirements: Windows 10 or 11, Rust 1.99.1 (the release `rust-toolchain.toml` pins), Visual Studio Build Tools (MSVC / C++), and the Windows SDK.

```bash
git clone https://github.com/cosmokud/rust-bg-muter.git
cd rust-bg-muter
cargo build
cargo test
```

The release binary is written to `target/release/rust-bg-muter.exe` by `cargo build --release`.

`cargo test` runs against a directory of the tests' own rather than your real settings: a test binary resolves its config directory to `%TEMP%\rust-bg-muter-test-<pid>` instead of `%APPDATA%\rust-bg-muter`, so a test run never rewrites the exclusion lists of an app you already have running. Set `RUST_BG_MUTER_CONFIG_DIR` to point a run at a directory of your choice.

To share one build cache between checkouts — and between worktrees — set `git config bgm.targetDir <path>`: the repo's `post-checkout` hook writes it into a git-ignored `.cargo/target-dir.local.toml`, which `.cargo/config.toml` includes. With nothing set, cargo uses the checkout's own `target/`. The same hook honours `git config bgm.releaseLto thin` for a faster release link on a workstation; what ships keeps the `lto = true` the manifest declares.

See [README.md](README.md) for app usage and configuration keys.

## Making changes

1. Create a branch from `main` with a short, descriptive name:

   ```bash
   git checkout -b fix/short-description
   ```

2. Make your changes. Follow the existing code style and keep changes focused: one issue per branch. Leave the formatting of code you did not touch alone, unless the formatting is the change itself: a whole-tree sweep is a branch of its own, one `cargo fmt --all` commit with nothing else in it.
3. Format, check, and test before you push:

   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   ```

If a check fails, fix it before opening a pull request. `rust-toolchain.toml` pins the toolchain, so cargo, rustfmt and clippy answer the same on every machine as they do in CI. `manual-ci.yml` runs those three on demand — started by hand from the Actions tab, against whichever branch or commit is chosen there — and is given nothing but read access to the repository.
4. Write a clear commit message in the present tense, for example `Fix mute state lost on theme change` rather than `Fixed...`.
5. Push your branch and open a pull request against `main`.

## Checks, and what they cost

Measured warm, on the pinned toolchain: `cargo check --all-targets` is a few seconds, `cargo clippy --all-targets -- -D warnings` about the same, `cargo fmt --all -- --check` under two seconds, and `cargo test` runs 92 tests in under two seconds. A clean build is the expensive one, because the tree links the Windows bindings and the image decoder.

## Running one test

cargo matches a filter against a test's path, so a module name is a filter, and `--test <name>` picks one file under `tests/` rather than the whole target:

```bash
cargo test muter                 # the muting rules
cargo test --test no_path_leaks  # the path guard, one file
```

## The suite and the desktop

Most of the suite answers values and draws nothing: the muting rules (`muter`), the configuration file (`config`), the text field's editing rules (`field`) and the path guard. The settings dialog's layout and painting tests make a real window, so they want an interactive Windows session; a headless session has nothing for that window to answer, and a failure there is the session answering rather than the change.

One test carries `#[ignore]`: `settings_dialog::tests::dump_frame` writes a painted frame to `.tmp/frame.bin` for offscreen inspection, which is a development aid. Run it with `cargo test -- --ignored`.

## Hooks

`git config core.hooksPath .githooks` is what makes these run:

- `prepare-commit-msg` refuses a message that names a profile directory, which reaches every clone the way a path in a fixture does; `--no-verify` does not skip it.
- `pre-push` runs `cargo test --test no_path_leaks` — the tracked tree and the messages in its history — and stops the push on a failure.
- `post-checkout` writes the machine-local cargo files described above. Unset, it writes nothing.

## Opening a pull request

- Fill in the pull request template (what you changed, why, and how you tested it).
- Link the issue it fixes, if there is one (for example, `Fixes #12`).
- Keep the pull request focused. If you notice something unrelated, open a separate issue or pull request for it.
- A maintainer will review your pull request. You may be asked to make changes; just push new commits to the same branch.

## Reporting bugs

Use the bug report issue template and include:

- What you expected to happen and what happened instead.
- Steps to reproduce the problem.
- App version, Windows version, and anything unusual about your setup.
- Screenshots, logs, or sample files if they help (remove private data).

## Suggesting features

Use the feature request issue template and describe the problem your idea solves, what you would like to happen, and any alternatives you considered.

## Questions

If you are unsure about anything, just ask in an issue. Beginners are welcome, and no question is too small.

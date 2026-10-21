# Contributing to Mycel

Mycel is built by a small team. This page is everything a new contributor
needs to go from "invited" to "first pull request".

## 1. One-time setup

1. **Accept the invitation.** Open the e-mail from GitHub, or go to
   <https://github.com/emrezorlu1239/mycel/invitations>, and click *Accept*.
2. **Install the tools.**
   - [Git](https://git-scm.com/downloads)
   - [Rust](https://rustup.rs) (stable, 1.88 or newer)
   - Windows only: *Visual Studio Build Tools* with the
     "Desktop development with C++" workload
3. **Tell Git who you are** (use the e-mail on your GitHub account so commits
   are linked to you):
   ```bash
   git config --global user.name  "Your Name"
   git config --global user.email "you@example.com"
   ```
4. **Clone and build.** Keep the path free of spaces and non-ASCII characters
   (e.g. `C:\Dev\mycel`), otherwise some build tools fail.
   ```bash
   git clone https://github.com/emrezorlu1239/mycel.git
   cd mycel
   cargo build --release
   cargo test
   ```
   The first build downloads dependencies and takes a few minutes. If
   `cargo test` ends with all tests passing, you are ready.
5. **Sign in for pushing.** The first `git push` opens a browser window to log
   in to GitHub (Git Credential Manager on Windows). Alternatively install the
   [GitHub CLI](https://cli.github.com) and run `gh auth login`.

## 2. Everyday workflow

Never commit straight to `main`. Every change goes through a branch and a pull
request, so the other person sees it and CI tests it first.

```bash
git checkout main
git pull                               # start from the latest code
git checkout -b feat/short-description # one branch per piece of work
# ... edit, then:
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
git add -A
git commit -m "Add relay discovery via Kademlia"
git push -u origin feat/short-description
```

Then open a pull request on GitHub. Merge it once CI is green and the other
person has had a look.

Branch name prefixes: `feat/` new feature, `fix/` bug fix, `docs/`
documentation, `test/` tests only, `refactor/` no behaviour change.

## 3. Conventions

- English for code, comments, commit messages and documentation.
- Commit messages: short imperative subject line ("Add …", "Fix …"), details in
  the body if needed.
- Networking changes should keep `scripts/netsim/netsim.sh` passing (Linux,
  needs root; CI runs it on every push).
- Read [docs/architecture.md](docs/architecture.md) before changing
  `crates/mycel-core`.

## 4. Project map

| Path | What |
|------|------|
| `crates/mycel-core` | networking core: identity, invites, protocol, libp2p node |
| `crates/mycel-cli` | `mycel` command-line client |
| `apps/desktop` | Tauri desktop app (Phase 1) |
| `scripts/netsim` | simulated internet for NAT tests |
| `docs/` | architecture, roadmap, test reports |

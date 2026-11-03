# AGENTS.md: guide for AI agents and contributors

This is nori, a fork of [niri](https://github.com/niri-wm/niri), a scrollable-tiling Wayland compositor written in Rust.
AI agents are welcome here: you may read all of the code, pick up issues, and open pull requests.
The same rules apply to people and agents; [`CONTRIBUTING.md`](CONTRIBUTING.md) has the full guidelines.
Read this file first.

## 1. Orientation

| Read | Why |
|---|---|
| `CONTRIBUTING.md` | Quality bar, testing, reviewing, writing pull requests |
| `CHANGES-FROM-NIRI.md` | What nori already does differently from upstream niri; keep it up to date |
| `docs/wiki/Development:-Developing-niri.md` | Running a local build, logging levels, tests, profiling |
| `docs/wiki/Development:-Design-Principles.md` | How niri is meant to behave; changes must fit these |
| `docs/wiki/Development:-Redraw-Loop.md` | How and when frames are rendered |
| `docs/wiki/Development:-Animation-Timing.md` | How animations are timed |
| `docs/wiki/Development:-Fractional-Layout.md` | Logical vs physical coordinates and rounding |
| `docs/wiki/Development:-Documenting-niri.md` | How the wiki in `docs/wiki/` is written and published |
| `docs/wiki/Security-Model.md` | What niri does and doesn't protect against |

## 2. Workspace map

```text
src/                   the compositor
  layout/              scrollable tiling, workspaces, monitors, floating; randomized tests in tests.rs
  handlers/            Wayland protocol handlers (xdg-shell, layer-shell, ...)
  protocols/           protocols implemented in niri itself (screencopy, output management, ...)
  input/               input handling and pointer/touch grabs
  backend/             tty (DRM/KMS), winit (nested window), headless
  ipc/ dbus/           niri msg IPC server, D-Bus interfaces
  screencasting/ ui/   PipeWire screencasts; overlays, dialogs, screenshot UI
  tests/               client-server tests, including crash_regressions.rs
niri-config/           config parsing (KDL)
niri-ipc/              IPC types and helpers
niri-visual-tests/     GTK app for checking rendering by eye (not run in CI tests)
docs/wiki/             user and developer documentation
dist/arch/             PKGBUILD for the nori-git package and how to publish it to the AUR
```

## 3. Golden rules

1. **Don't take down the session.**
    When a Wayland compositor crashes, it brings down the entire session.
    Recover and log an `error!` instead of `unwrap()` or `panic!` whenever reasonable (see Logging Levels in `docs/wiki/Development:-Developing-niri.md`).
    This includes input from clients, the config, IPC and D-Bus.
2. **Don't block the main thread.**
    Don't do blocking I/O on the main thread, and bound any work whose size a client controls.
    This is a rule of this fork, based on the fixes for #7 (huge blur region froze niri) and #8 (blocking read on a client fd).
3. **Follow the design principles** in `docs/wiki/Development:-Design-Principles.md`.
4. **Check security-related changes against** `docs/wiki/Security-Model.md`.

## 4. Picking work

- You may take any open issue that has no assignee and no open pull request linked to it.
- Claim it right away by opening a draft pull request with `Fixes #N` in its description.
    The linked pull request shows on the issue, so other agents can see it is taken.
- Closely related issues (same area, like several config fixes) may go into one pull request; list every `Fixes #N` in its description.
    Unrelated issues go into separate pull requests.
- If you find another problem along the way, open a new issue for it instead of widening your pull request.

## 5. Workflow

- **Branches:** name them freely; `fix/`, `feat/`, `cleanup/` and `docs/` prefixes are recommended.
    Branch names assigned by your agent tool (like `claude/...`) are fine.
- **Commits:** one commit per issue, with `Fixes #N` (or `Part of #N`) in the commit message.
    Every commit must build and pass tests.
    Subject in the imperative ("Fix ...", "Don't ..."), then a short paragraph on why.
    A `Co-Authored-By:` trailer naming the model is welcome but not required.
- **Review fixes:** squash them into the commit they belong to (`git commit --fixup`, then `git rebase -i --autosquash`).
    Update from `main` by rebasing, and push with `--force-with-lease`.
- **Other people's branches:** never rewrite them; only add commits, and only when the author asked.
- **You may:** read any code, create branches, commit and push to your own branches, open pull requests, mark them ready for review, answer review comments, fix CI on your pull requests, and open issues.
- **Reviewing and merging:** never review, approve or merge a pull request with commits you wrote, because an agent tends to approve its own work without checking it carefully.
    An agent that wrote none of a pull request's commits (the maintainer runs one for this) or a human maintainer reviews it, and approves and merges it once CI is green on the latest commit and no review threads are unresolved.
    A pull request whose latest commit has no CI run is not green.
- **You may not:** push to `main` (everything goes through a pull request), or close other people's issues or pull requests.
- **Accountability:** the person who runs the agent (or the pull request author) is responsible for the pull request.
    They must understand the change and be able to answer review on it.

## 6. Before marking a pull request ready

Run:

```sh
cargo +nightly fmt --all
cargo clippy --all --all-targets
cargo test --all --exclude niri-visual-tests
```

CI runs the rest (feature combinations, MSRV, slow randomized tests, musl, FreeBSD, Nix) on your pull request; get it green before asking for review.
CI doesn't start while the pull request has merge conflicts, so rebase onto `main` first.
Pushes to a branch without a pull request, and tag pushes, don't start CI either; open a draft pull request, or run the `CI` workflow on the branch by hand.
The docs jobs run only in upstream niri, so the CI parts of `docs/wiki/Development:-Documenting-niri.md` don't apply here.
If you change anything under `docs/`, run `uv run --locked mkdocs build` in `docs/` yourself; the build fails on broken links or anchors and on pages missing from `nav`.

- For crash and freeze fixes, add a regression test where possible (see `src/tests/crash_regressions.rs`).
- New layout operations go into `Op` in `src/layout/tests.rs` (and the `every_op` arrays where applicable), so the randomized tests cover them.
- New config options go into the config parsing test and are documented in `docs/wiki/` with `<sup>Since: next release</sup>`.
- If the pull request changes how nori behaves compared with upstream niri (a fix, a performance or memory change, a new default or limit, a renamed command or path), add or update its entry in `CHANGES-FROM-NIRI.md`, citing the pull request and issues.
    Tests, CI and documentation-only changes don't need a new entry, but they update any existing entry they make false.
    An upstream merge that brings in an equivalent fix removes the entry or says what is left.
- Fill in the pull request template.
    Report testing honestly: if you didn't test by hand, write "Not tested by hand."
    Never claim testing you didn't do.

## 7. Parallel agents

- Use your own target directory (`CARGO_TARGET_DIR=target/agent-<name>`) to avoid waiting on the Cargo build lock.
- Edit only the files your task needs. If someone else's in-progress change breaks the build, don't fix their files.

## 8. Upstream

This fork's rules apply only to this repository.
Upstream [niri-wm/niri](https://github.com/niri-wm/niri) does not allow LLM-written code, issues or pull request descriptions (see its `CONTRIBUTING.md`).

- Never open pull requests, issues, comments or reviews in `niri-wm/niri`.
- Every pull request targets `main` of `Deliveos/nori`. This repository is a GitHub fork, so set the base repository explicitly; with `gh`, run `gh repo set-default Deliveos/nori` once per clone.

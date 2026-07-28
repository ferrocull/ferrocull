# Contributing to Ferrocull

Bug reports, feature requests and pull requests are all welcome. Fork it, branch, and open a pull request. Small focused changes get merged fastest.

If you are about to start something large, an issue first will save you work, because scope is the one thing worth agreeing on before the code exists.

## Reporting a bug

Use the [bug report form](https://github.com/ferrocull/ferrocull/issues/new/choose). It asks for your OS, the camera or card, and the file format, because culling bugs almost always turn out to be specific to one of those three. A report without them usually cannot be reproduced.

Run with `RUST_LOG=debug` and paste the relevant output if the app misbehaves rather than crashes.

## Finding something to work on

Issues carry one of five triage labels:

| Label             | Meaning                                                        |
| ----------------- | -------------------------------------------------------------- |
| `needs-triage`    | not looked at yet, do not start on it                          |
| `needs-info`      | waiting on the reporter, blocked                               |
| `ready-for-agent` | specified in enough detail to hand to a coding agent, or human |
| `ready-for-human` | needs judgement that a specification cannot carry              |
| `wontfix`         | decided against                                                |

`ready-for-agent` is a statement about how well specified the issue is, not about who may take it. Anyone can pick one up. `ready-for-human` is the opposite signal: the issue involves a design call, a trade-off, or something you have to see with your own eyes.

## Getting set up

Install the native dependencies first: they are listed in the [README's install section](README.md#install), along with the libjpeg-turbo version caveat that catches most people out. [`.github/workflows/ci.yml`](.github/workflows/ci.yml) shows exactly how CI installs them.

Rust nightly is pinned by `rust-toolchain.toml`, so rustup fetches the right toolchain on the first build. After that `cargo run` behaves as you would expect.

You do not need a camera, or even a card, to work on Ferrocull. Add a folder of images as a source and everything except device discovery behaves the same.

## Before you push

Run the gate. CI runs the same three commands and clippy fails on any warning, so this is the whole difference between a green pull request and a red one:

```sh
cargo fmt
cargo clippy --workspace --all-targets
cargo test --workspace
```

Prefer fixing what clippy suggests. If a lint is wrong for a particular piece of code, suppress it with `#[expect(clippy::...)]` and a comment saying why, never `#[allow]`. An `#[expect]` warns once it stops being needed, which is what you want.

## Commits

[Angular conventional commits](https://www.conventionalcommits.org/), with the types `build`, `ci`, `docs`, `feat`, `fix`, `perf`, `refactor`, `test`.

Follow git's 50/72 convention: subject of 50 characters or fewer, imperative mood, body wrapped at 72. Keep the body about why the change was made, since the diff already shows what changed. Skip the body when the subject says it all.

```
fix: keep the burst badge visible while collapsed

The badge was drawn under the thumbnail overlay, so a collapsed
burst looked identical to a single frame at small cell sizes.
```

## Pull requests

Every pull request, whoever or whatever wrote it:

- **You can explain the diff in review.** If you cannot say why a line is there, it is not ready.
- **Tests come with the change, and they fail without it.** Check that they do. A test that passes on an unfixed bug is worse than no test.
- **The gate is green.** Format, clippy, tests.
- **UI changes come with a screenshot.** It is a visual tool, and a description of a layout change is not reviewable.

Link the issue if there is one. If the change grew past what the issue described, say so in the description rather than quietly expanding scope.

## Code style

[`AGENTS.md`](AGENTS.md) is the contract that code in this repository follows: naming, error handling, when to `expect` and when to propagate, how the UI and the core are structured. It is written for coding agents, but the rules apply to everyone. Read it before writing code.

[`CONTEXT.md`](CONTEXT.md) is the glossary. Use the words it defines, in code and in prose. A pull request that calls a burst a sequence, or tagging picking, creates work for whoever reads it next.

## AI-assisted contributions

Use whatever tools you like. Nobody is going to ask whether an agent wrote your patch, and there is no disclosure to make, because the review looks at the code rather than at where it came from.

The bar in the previous section is the whole policy. Read what you are submitting and it will not be a problem.

## License

By contributing you agree that your contributions are licensed under the [Apache License 2.0](LICENSE), as section 5 of that license already provides. There is no separate agreement to sign.

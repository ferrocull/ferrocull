# Ferrocull

A FOSS culling tool for photographers, in the Photo Mechanic tradition, but open source. Built in Rust with [iced](https://iced.rs/).

## Status

Alpha. Core culling (rating, color labels, tag/selection, RAW+JPEG grouping, burst detection, full-screen preview, compare mode) and ingest (pattern-based renaming, SHA256 verification, multi-destination, post-hooks, XMP sidecar export) are implemented. See [GitHub issues](https://github.com/ferrocull/ferrocull/issues) for what's next.

## Build

```sh
cargo build --release
```

Requires Rust nightly.

## Install (Linux)

`make install` builds the release binary and installs it alongside a desktop
entry and hicolor icons, so Ferrocull shows up in the application launcher.
Where it lands is controlled by `prefix`, following the GNU install layout:

```sh
make install prefix=$HOME/.local        # per-user, no root
make build && sudo make install         # system-wide, prefix defaults to /usr/local
```

Build as your own user: `make install` refuses to compile under `sudo`, which
would leave root-owned artifacts in `target/`.

Requires `~/.local/bin` on your `PATH` for the per-user install. `PREFIX` works
as an alias for `prefix`, and the usual `bindir`/`datadir` overrides are
honoured. `make uninstall` takes the same variables and removes what was
installed.

Packagers can stage the tree with `make install prefix=/usr DESTDIR=$pkgdir`;
`DESTDIR` also suppresses the desktop and icon cache refresh.

## Contributing

Ferrocull is a personal project first, but pull requests and suggestions are welcome. There's no `CONTRIBUTING.md` yet, for now, open an issue to discuss anything non-trivial before sending a PR.

## License

[Apache-2.0](./LICENSE)

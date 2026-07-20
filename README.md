# Ferrocull

A FOSS culling tool for photographers, in the Photo Mechanic tradition, but open source. Built in Rust with [iced](https://iced.rs/).

## Status

Alpha. Core culling (rating, color labels, tag/selection, RAW+JPEG grouping, burst detection, full-screen preview, compare mode) and ingest (pattern-based renaming, SHA256 verification, multi-destination, post-hooks, XMP sidecar export) are implemented. See [GitHub issues](https://github.com/remigastaldi/ferrocull/issues) for what's next.

## Build

```sh
cargo build --release
```

Requires Rust nightly.

## Contributing

Ferrocull is a personal project first, but pull requests and suggestions are welcome. There's no `CONTRIBUTING.md` yet, for now, open an issue to discuss anything non-trivial before sending a PR.

## License

[Apache-2.0](./LICENSE)

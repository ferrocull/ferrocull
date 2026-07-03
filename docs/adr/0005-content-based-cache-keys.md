# Content-based cache keys for thumbnails

The thumbnail cache is keyed by a content hash (`cache::cache_key_from_disk`), not by file path. Replugging the same SD card on a different mount point, copying a card to disk, or renaming a folder all preserve cache hits — the bytes are the same, the key is the same.

The cost is the CPU and I/O of hashing every file at lookup time. Mitigated by reading incrementally with early termination (2MB chunks) so we usually hash only what's needed to disambiguate.

Path-based keys were the obvious default but fail the central use case: photographers ingest from cards that mount at different paths every session, and a path-based cache would re-thumbnail everything every time.

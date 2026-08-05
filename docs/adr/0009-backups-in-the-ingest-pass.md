# Backup copies are teed from the source stream, not verified by re-read

Ingest streams each source file once and tees the stream to the primary destination and every backup destination; the SHA-256 checksum is computed from that single source stream and shared by all copies. No copy is verified by reading the destination back.

Considered alternative: copying backups after ingest by re-reading the primary destination and comparing checksums. Rejected because it doubles the I/O and serializes wall time, while the "verification" it appears to provide is served from the page cache — it verifies memory, not media. Genuine read-back verification would require dropping caches (O_DIRECT), which no mainstream ingest tool does either.

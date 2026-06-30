# File Candidate Fixture Corpus

This corpus is for `filesystem.find_file_candidates` validation only.

It is intentionally separate from `tests/fixtures/transfer-corpus/`, which is size, throughput, scheduler, and MicroFlowGroup oriented. File-candidate tests need stable filenames, nesting, hidden entries, redacted-location checks, and metadata-only assertions rather than large generated payloads.

`app-data/shared/` mirrors the receiver-local app-owned directory used by the executor for the safe `pastey_shared` scope.

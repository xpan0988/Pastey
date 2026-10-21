# Native NAT traversal experiment (research only)

Standalone crate used for `../native-nat-traversal.md`. Not part of the Pastey
build or workspace (`[workspace]` in its own `Cargo.toml`), and nothing in the
app depends on it.

- `stun_probe.py` sends STUN binding requests from one UDP socket to Google and
  Cloudflare STUN servers and reports whether the NAT mapping is endpoint
  independent. Run it with `python3 -I stun_probe.py`.
- `src/main.rs` runs two iroh 1.3 endpoints in one process, using n0's public
  relays. Scenarios:
  - `direct` sends a pinned-peer, digest-checked stream, gated on a direct path.
  - `relay-only` measures that the gate withholds application data.
  - `unpinned` checks that a valid but unpaired key is refused.

  Run it with `cargo run --release -- <scenario> [MiB]`.

Both endpoints run on one machine, so a "direct" result here is not cross-NAT
evidence. Phases 1–3 of the report's PoC plan run it on two devices.

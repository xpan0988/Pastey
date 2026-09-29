# Retired: MicroDuck Gate B qualification route

Not compiled, not tested, not packaged. Kept for reference only.

This directory holds the MicroDuck Stage 9 qualification route (qualification
runs 001–009j), which is void. It pinned one device's implementation inside
Pastey Core:

- `ProfilePinsV1` compiled pins (MuJoCo version, Python environment and
  executable hashes, ONNX runtime hash, exactly one policy hash);
- walk/stand policy-slot checks;
- yaw-projected reference displacement and 500 ms settling analysis;
- the owned native rebuild, Python environment digest and installation
  checks, and the Gate B evidence bundle / qualification record.

Changing the device's policy (for example walk+stand to velstand) required
Core code changes, which violates the Core/binding boundary.

Pastey Core now keeps only an opaque `implementation_fingerprint`: a binding
reports an ordered `name -> sha256` map, Core never interprets the names, and
any change makes the qualification unusable until a new record is issued.
Device-specific qualification belongs to a device binding and its own tooling.

Contents: `src/qualification.rs`, `src/stage9_tests.rs` (Rust, formerly under
`src-tauri/src/physical/`), `scripts/` (Gate B preparation/environment
scripts and their Python tests, plus the environment-digest fixture) and
`native/` (the profile and environment pin files).

The Stage 8 native-fence mechanism (robotd overlay, native lane) and the Gate A
supervisor are separate and are retired in the next step.

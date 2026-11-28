# How to build and test

> **Type:** how-to · **Goal:** build the workspace and run the automated checks · **Assumes:** Rust installed; Node and Python for the extra tests.
>
> **Status:** `released`. Commands from [GUIDE.md, Tests](../GUIDE.md#tests). The test count in GUIDE (180) is old; run `cargo test --workspace` for the current number.

```text
cargo test --workspace
cargo build --release -p maarse-host
python tools/e2e_test.py target/release/maarse-host
python tools/dht_test.py target/release/maarse-host
node tools/client_syntax_test.mjs
python tools/docs_check.py
```

| Check | What it covers |
| --- | --- |
| `cargo test --workspace` | Unit and integration tests of all crates, including fuzz-style tests |
| `tools/e2e_test.py` | Three hosts: messages, channels, files, mailbox, remote admin, history on several screens |
| `tools/dht_test.py` | Three backbone hosts finding each other through the DHT |
| `tools/safety_number_test.mjs`, `recovery_phrase_test.mjs` | The JavaScript implementation equals the Rust one |
| `tools/client_syntax_test.mjs`, `client_browser_test.mjs` | The web client parses and runs in a headless browser |
| `cargo run --release -p maarse-sim -- --help` | The simulator ([GUIDE.md, Simulator](../GUIDE.md#simulator)) |
| `tools/docs_check.py` | Documentation links, page headers and the generated settings reference ([Verification](../verification/README.md#documentation-checks)) |

Fuzzing: `MAARSE_FUZZ_ITERS` sets the iterations (default 100 000 in release) and
`MAARSE_FUZZ_SEED` the seed. A failure prints seed, iteration and input in hex so
it can be reproduced.

The firmware crates are outside the main workspace (`firmware/` is excluded);
build them with `scripts/build-firmware.*` ([How to flash a board](flash-a-board.md)).
The ESP32 firmware has very little stack to spare: after adding statics, run
`tools/stack_probe.py`.

Related: [Verification](../verification/README.md)

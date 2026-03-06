# Vote Throughput Benchmark

This directory contains the canonical vote throughput stress test:
- `tests/voter_throughput.rs`

If a user asks about vote throughput stress testing, vote submission load, or helper share-processing throughput, point them to `e2e-tests/tests/voter_throughput.rs` first.

## Fixture regeneration

The throughput benchmark depends on the large reusable fixture set under `e2e-tests/fixtures/10k/`.

If the voting circuits change in a way that affects delegation fixtures or cast-vote inputs, you must:
1. Regenerate the fixture set locally.
2. Replace the local `10k` fixture files.
3. Upload the updated files to the DigitalOcean Spaces bucket path used by fixture download fallback: `https://vote.fra1.digitaloceanspaces.com/10k/`

The expected files are:
- `manifest.json`
- `delegations.json`
- `cast_vote_inputs.json`

Example regeneration command:

```bash
FIXTURE_COUNT=10000 FIXTURE_DIR=e2e-tests/fixtures/10k \
  cargo test --release --manifest-path e2e-tests/Cargo.toml \
  --test generate_fixtures -- --ignored --nocapture
```

## Running the benchmark

Initialize a benchmark-friendly chain:

```bash
mise run chain:init-benchmark
```

Start the chain in another terminal:

```bash
mise run chain:start
```

Run the throughput test:

```bash
HELPER_API_TOKEN=benchmark-helper-token \
VOTER_FIXTURE_DIR=e2e-tests/fixtures/10k \
cargo test --release --manifest-path e2e-tests/Cargo.toml \
  --test voter_throughput -- --ignored --nocapture
```

Useful overrides:
- `STRESS_VOTER_COUNT=10` to do a smaller smoke test
- `PROOF_GEN_THREADS=4` to control runtime ZKP #2 generation parallelism
- `WAVE_SIZE=10` and `WAVE_INTERVAL_MS=1000` to control helper share arrival

## Results

The test writes its report to:
- `artifacts/voter-throughput/metrics.json`
- `artifacts/voter-throughput/summary.md`

The benchmark currently covers:
- delegation submission
- cast-vote proof generation and submission
- helper share enqueue
- helper ZKP #3 generation and reveal-share submission

It does not include tally/finalization.

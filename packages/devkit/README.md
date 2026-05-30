# stellar-devkit

Developer toolkit for the Stellar Fee Tracker. Provides utilities for testing, mocking, and simulating Stellar network behaviour without hitting live infrastructure.

## Scope

`stellar-devkit` is a standalone testing and simulation package. It must not import from `stellar-core` or any live-network crate. All functionality is self-contained and intended for use in `[dev-dependencies]` only.

## Boundary Rules

- No imports from `packages/core`
- No live Horizon API calls
- No database connections
- All external I/O must be injectable or mockable

## Modules

| Module | Description |
|---|---|
| `harness` | Mock Horizon server and pre-built fee scenario fixtures |
| `harness::scenarios` | JSON scenario files and runtime loader |
| `simulation` | Fee models, network-load generators, congestion predictors |
| `analysis` | Percentile stats, spike classification, rolling window |
| `cli` | Replay, export, and benchmark CLI stubs |
| `types` | Shared types: `FeeRecord`, `Scenario`, `SimResult` |
| `error` | `DevkitError` unified error enum |

## Simulation

The `simulation` module provides fee modelling and network-load generation without any live-network dependencies.

### `FeeModelConfig` fields

| Field | Type | Description |
|---|---|---|
| `base_fee` | `u64` | Minimum fee (stroops) used as the simulation floor |
| `surge_multiplier` | `f64` | Fee multiplier applied when the network is congested |
| `congestion_threshold` | `f64` | Load ratio (0.0–1.0) above which surge pricing activates |

### Example usage

```rust
use stellar_devkit::simulation::{FeeModel, NetworkLoad};

let load = NetworkLoad::constant(0.85);          // 85 % utilisation
let result = FeeModel::run(&load, base_fee: 100, surge_multiplier: 2.0, congestion_threshold: 0.8);
println!("recommended fee: {} stroops", result.recommended_fee);
```

### Output format (`SimResult`)

| Field | Type | Description |
|---|---|---|
| `recommended_fee` | `u64` | Suggested fee for the simulated conditions |
| `congested` | `bool` | Whether surge pricing was triggered |
| `load_ratio` | `f64` | Network utilisation at simulation time |

## Running

```bash
# Run all devkit tests
cargo test -p stellar-devkit

# Run a specific test file
cargo test -p stellar-devkit --test harness_congested
```

## Mock Horizon Server

The harness exposes canned `GET /fee_stats` payloads through `HorizonMock` and the JSON fixtures in `src/harness/scenarios/`.

```bash
# Start with the baseline fixture
cargo test -p stellar-devkit --test harness_normal -- --nocapture

# Swap to a higher-pressure fixture
cargo test -p stellar-devkit --test harness_congested -- --nocapture
```

Scenario flags map directly to the fixture you load in your test setup:

- `normal` for a low-fee baseline
- `congested` for sustained high-fee demand
- `spike` for a sudden short-lived fee jump
- `recovery` for a return from congestion toward baseline

```rust
use std::path::Path;

use stellar_devkit::harness::{
    horizon_mock::HorizonMock,
    scenarios::load_from_file,
};

let payload = load_from_file(Path::new("src/harness/scenarios/spike.json"))?;
let mock = HorizonMock::new(payload);
assert!(mock.fee_stats_payload().contains("\"scenario\": \"spike\""));
```

## CLI

The devkit ships with a set of subcommands for driving scenarios from the command line.

### Usage

```bash
devkit <SUBCOMMAND> [OPTIONS]
```

### Subcommands

| Subcommand | Description |
|---|---|
| `replay` | Replay recorded fee scenarios from a SQLite database |
| `export` | Export fee data to CSV |
| `benchmark` | Run performance benchmarks against the fee pipeline |
| `mock` | Serve mock Horizon `/fee_stats` responses |
| `simulate` | Run a network-load simulation and print results |

### Examples

```bash
# Replay fee records from a local SQLite file
devkit replay ./fees.db

# Export fee data to CSV
devkit export ./fees.db --output fees.csv

# Run benchmarks
devkit benchmark --samples 1000

# Start the mock server
devkit mock --port 8080 --scenario spike
```

## Adding to Your Crate

```toml
[dev-dependencies]
stellar-devkit = { path = "../devkit" }
```

```toml
[dev-dependencies]
stellar-devkit = { path = "../devkit" }
```

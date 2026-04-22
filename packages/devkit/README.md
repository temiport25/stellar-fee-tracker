# stellar-devkit

Developer toolkit for the Stellar Fee Tracker. Provides utilities for testing, mocking, and simulating Stellar network behaviour without hitting live infrastructure.

## Modules

- **harness** — Test harness with a Horizon mock server and pre-built scenario runners.
- **simulation** — Fee models, network-load generators, and congestion predictors for local simulation.

## Usage

Add `stellar-devkit` to your `[dev-dependencies]` and import the modules you need.

```toml
[dev-dependencies]
stellar-devkit = { path = "../devkit" }
```

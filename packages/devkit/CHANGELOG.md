# Changelog

All notable changes to the devkit package will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2025-01-01

### Added
- `harness` module: `HorizonMock` server for replaying Stellar fee scenarios in tests
- `scenario_loader`: loads and resolves scenario JSON files from disk
- `simulation` module: deterministic fee and congestion simulation utilities
  - `FeeModel`: models fee dynamics under varying network conditions
  - `CongestionPredictor`: predicts congestion levels from historical fee data
  - `NetworkLoad`: represents snapshot of current network transaction load
- `analysis` module: statistical analysis helpers
  - `Percentile`: computes configurable percentile values over fee samples
  - `RollingWindow`: maintains a fixed-size sliding window of observations
  - `SpikeClassifier`: classifies fee spikes relative to a rolling baseline
- `cli` module: CLI stubs for developer tooling
  - `benchmark` sub-command stub
  - `export` sub-command stub
  - `replay` sub-command stub
- `types` module: shared domain types (`FeeRecord`, `Scenario`, `SimResult`)
- `error` module: unified `DevkitError` enum covering simulation, harness, analysis, and I/O errors

/// A single fee observation captured from the Stellar network at a point in time.
///
/// Used as the primary data unit flowing through analysis and simulation pipelines.
pub struct FeeRecord;

/// A named test scenario for the devkit harness.
///
/// Scenarios encapsulate a snapshot of network conditions (fee levels, congestion,
/// transaction volume) and are loaded from JSON files under `src/harness/scenarios/`.
pub struct Scenario;

/// The result of a completed simulation run.
///
/// Aggregates outputs produced by the simulation engine, including predicted fee
/// levels, congestion classifications, and any spike events detected.
pub struct SimResult;

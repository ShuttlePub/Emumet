use ferroid::generator::AtomicSnowflakeGenerator;
use ferroid::time::MonotonicClock;
use std::sync::OnceLock;
use std::time::Duration;

ferroid::define_snowflake_id!(
    EmumetSnowflake, u64,
    reserved: 1,
    timestamp: 41,
    machine_id: 10,
    sequence: 12
);

/// Custom epoch: 2026-01-01T00:00:00Z (Unix timestamp = 1767225600 seconds)
const EMUMET_EPOCH: Duration = Duration::from_secs(1767225600);
const EMUMET_EPOCH_MS: u64 = 1767225600 * 1000;

type Generator = AtomicSnowflakeGenerator<EmumetSnowflake, MonotonicClock>;

static GENERATOR: OnceLock<Generator> = OnceLock::new();

/// Maximum worker ID (10-bit field: 0-1023).
const MAX_WORKER_ID: u64 = (1 << 10) - 1;

pub fn init_generator(worker_id: u64) {
    assert!(
        worker_id <= MAX_WORKER_ID,
        "WORKER_ID must be 0-{MAX_WORKER_ID}, got {worker_id}"
    );
    let clock = MonotonicClock::with_epoch(EMUMET_EPOCH);
    let gen = AtomicSnowflakeGenerator::new(worker_id, clock);
    GENERATOR
        .set(gen)
        .unwrap_or_else(|_| panic!("Snowflake generator already initialized"));
}

pub fn generate_id() -> i64 {
    let gen = GENERATOR
        .get()
        .expect("Snowflake generator not initialized. Call init_generator() first.");
    let id: EmumetSnowflake = gen.next_id(|_| std::thread::yield_now());
    id.to_raw() as i64
}

pub fn extract_timestamp_ms(raw: i64) -> u64 {
    let ts_offset =
        ((raw as u64) >> EmumetSnowflake::TIMESTAMP_SHIFT) & EmumetSnowflake::TIMESTAMP_MASK;
    ts_offset + EMUMET_EPOCH_MS
}

/// Ensures the generator is initialized (idempotent, worker_id=0). Test-only.
#[cfg(any(test, feature = "test-utils"))]
pub fn ensure_generator_initialized() {
    let _ = GENERATOR.get_or_init(|| {
        let clock = MonotonicClock::with_epoch(EMUMET_EPOCH);
        AtomicSnowflakeGenerator::new(0, clock)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_id_is_positive() {
        ensure_generator_initialized();
        let id = generate_id();
        assert!(id > 0);
    }

    #[test]
    fn generate_ids_are_monotonic() {
        ensure_generator_initialized();
        let id1 = generate_id();
        let id2 = generate_id();
        assert!(id2 > id1);
    }

    #[test]
    fn extract_timestamp_returns_reasonable_value() {
        ensure_generator_initialized();
        let id = generate_id();
        let ts_ms = extract_timestamp_ms(id);
        assert!(ts_ms >= EMUMET_EPOCH_MS);
    }
}

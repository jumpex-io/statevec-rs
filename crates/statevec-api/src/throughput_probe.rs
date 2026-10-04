// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

#[cfg(feature = "throughput-probe")]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static RUNTIME_HOST_WITH_READ_TYPED_BY_UK_COUNT: AtomicU64 = AtomicU64::new(0);
    static RUNTIME_HOST_UPDATE_TYPED_BY_UK_COUNT: AtomicU64 = AtomicU64::new(0);
    static TYPED_TX_WITH_READ_TYPED_BY_UK_COUNT: AtomicU64 = AtomicU64::new(0);
    static TYPED_TX_UPDATE_TYPED_BY_UK_COUNT: AtomicU64 = AtomicU64::new(0);
    static TYPED_TX_UPDATE_OR_CREATE_TYPED_BY_UK_COUNT: AtomicU64 = AtomicU64::new(0);
    static DUMP_ONCE: AtomicBool = AtomicBool::new(false);

    #[inline(always)]
    fn load(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }

    #[inline(always)]
    fn store(counter: &AtomicU64, value: u64) {
        counter.store(value, Ordering::Relaxed);
    }

    #[inline(always)]
    fn add(counter: &AtomicU64, value: u64) {
        counter.fetch_add(value, Ordering::Relaxed);
    }

    pub struct RuntimeApiProbe;

    impl RuntimeApiProbe {
        pub fn reset() {
            store(&RUNTIME_HOST_WITH_READ_TYPED_BY_UK_COUNT, 0);
            store(&RUNTIME_HOST_UPDATE_TYPED_BY_UK_COUNT, 0);
            store(&TYPED_TX_WITH_READ_TYPED_BY_UK_COUNT, 0);
            store(&TYPED_TX_UPDATE_TYPED_BY_UK_COUNT, 0);
            store(&TYPED_TX_UPDATE_OR_CREATE_TYPED_BY_UK_COUNT, 0);
            DUMP_ONCE.store(false, Ordering::Relaxed);
        }

        pub fn dump_to_stderr_once(label: &str) {
            if DUMP_ONCE.swap(true, Ordering::Relaxed) {
                return;
            }
            eprintln!(
                concat!(
                    "[statevec_api_probe] label={label}",
                    " runtime_host_with_read_typed_by_uk_count={runtime_host_with_read_typed_by_uk_count}",
                    " runtime_host_update_typed_by_uk_count={runtime_host_update_typed_by_uk_count}",
                    " typed_tx_with_read_typed_by_uk_count={typed_tx_with_read_typed_by_uk_count}",
                    " typed_tx_update_typed_by_uk_count={typed_tx_update_typed_by_uk_count}",
                    " typed_tx_update_or_create_typed_by_uk_count={typed_tx_update_or_create_typed_by_uk_count}"
                ),
                label = label,
                runtime_host_with_read_typed_by_uk_count = load(&RUNTIME_HOST_WITH_READ_TYPED_BY_UK_COUNT),
                runtime_host_update_typed_by_uk_count = load(&RUNTIME_HOST_UPDATE_TYPED_BY_UK_COUNT),
                typed_tx_with_read_typed_by_uk_count = load(&TYPED_TX_WITH_READ_TYPED_BY_UK_COUNT),
                typed_tx_update_typed_by_uk_count = load(&TYPED_TX_UPDATE_TYPED_BY_UK_COUNT),
                typed_tx_update_or_create_typed_by_uk_count = load(&TYPED_TX_UPDATE_OR_CREATE_TYPED_BY_UK_COUNT),
            );
        }
    }

    #[inline(always)]
    pub(crate) fn on_runtime_host_with_read_typed_by_uk() {
        add(&RUNTIME_HOST_WITH_READ_TYPED_BY_UK_COUNT, 1);
    }

    #[inline(always)]
    pub(crate) fn on_runtime_host_update_typed_by_uk() {
        add(&RUNTIME_HOST_UPDATE_TYPED_BY_UK_COUNT, 1);
    }

    #[inline(always)]
    pub(crate) fn on_typed_tx_with_read_typed_by_uk() {
        add(&TYPED_TX_WITH_READ_TYPED_BY_UK_COUNT, 1);
    }

    #[inline(always)]
    pub(crate) fn on_typed_tx_update_typed_by_uk() {
        add(&TYPED_TX_UPDATE_TYPED_BY_UK_COUNT, 1);
    }

    #[inline(always)]
    pub(crate) fn on_typed_tx_update_or_create_typed_by_uk() {
        add(&TYPED_TX_UPDATE_OR_CREATE_TYPED_BY_UK_COUNT, 1);
    }
}

#[cfg(not(feature = "throughput-probe"))]
mod imp {
    pub struct RuntimeApiProbe;

    impl RuntimeApiProbe {
        pub fn reset() {}

        pub fn dump_to_stderr_once(_label: &str) {}
    }

    #[inline(always)]
    pub(crate) fn on_runtime_host_with_read_typed_by_uk() {}

    #[inline(always)]
    pub(crate) fn on_runtime_host_update_typed_by_uk() {}

    #[inline(always)]
    pub(crate) fn on_typed_tx_with_read_typed_by_uk() {}

    #[inline(always)]
    pub(crate) fn on_typed_tx_update_typed_by_uk() {}

    #[inline(always)]
    pub(crate) fn on_typed_tx_update_or_create_typed_by_uk() {}
}

pub use imp::RuntimeApiProbe;
pub(crate) use imp::{
    on_runtime_host_update_typed_by_uk, on_runtime_host_with_read_typed_by_uk,
    on_typed_tx_update_or_create_typed_by_uk, on_typed_tx_update_typed_by_uk, on_typed_tx_with_read_typed_by_uk,
};

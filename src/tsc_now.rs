// Copyright 2021 TiKV Project Authors. Licensed under Apache-2.0.

//! This module will be compiled when it's either linux_aarch64, linux_x86 or linux_x86_64.

use std::cell::UnsafeCell;
use std::time::Instant;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use std::fs::read_to_string;

static TSC_STATE: TSCState = TSCState {
    is_tsc_available: UnsafeCell::new(false),
    tsc_level: UnsafeCell::new(TSCLevel::Unstable),
    nanos_per_cycle: UnsafeCell::new(1.0),
};

struct TSCState {
    is_tsc_available: UnsafeCell<bool>,
    tsc_level: UnsafeCell<TSCLevel>,
    nanos_per_cycle: UnsafeCell<f64>,
}

unsafe impl Sync for TSCState {}

#[ctor::ctor]
unsafe fn init() {
    let tsc_level = TSCLevel::get();
    let is_tsc_available = match &tsc_level {
        TSCLevel::Stable { .. } => true,
        TSCLevel::Unstable => false,
    };
    if is_tsc_available {
        *TSC_STATE.nanos_per_cycle.get() = 1_000_000_000.0 / tsc_level.cycles_per_second() as f64;
    }
    *TSC_STATE.is_tsc_available.get() = is_tsc_available;
    *TSC_STATE.tsc_level.get() = tsc_level;
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

#[inline]
pub(crate) fn is_tsc_available() -> bool {
    unsafe { *TSC_STATE.is_tsc_available.get() }
}

#[inline]
pub(crate) fn nanos_per_cycle() -> f64 {
    unsafe { *TSC_STATE.nanos_per_cycle.get() }
}

#[inline]
pub(crate) fn current_cycle() -> u64 {
    match unsafe { &*TSC_STATE.tsc_level.get() } {
        TSCLevel::Stable(..) => tsc(),
        TSCLevel::Unstable => panic!("tsc is unstable"),
    }
}

enum TSCLevel {
    Stable(u64),
    Unstable,
}

impl TSCLevel {
    fn get() -> TSCLevel {
        if !is_tsc_stable() {
            return TSCLevel::Unstable;
        }

        let (cps, ..) = cycles_per_sec();
        TSCLevel::Stable(cps)
    }

    #[inline]
    fn cycles_per_second(&self) -> u64 {
        match self {
            TSCLevel::Stable(cps) => *cps,
            TSCLevel::Unstable => panic!("tsc is unstable"),
        }
    }
}

/// If linux kernel detected TSCs are sync between CPUs, we can
/// rely on the result to say tsc is stable so that no need to
/// sync TSCs by ourselves.
#[cfg(target_arch = "aarch64")]
fn is_tsc_stable() -> bool {
    true
}
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn is_tsc_stable() -> bool {
    let clock_source =
        read_to_string("/sys/devices/system/clocksource/clocksource0/available_clocksource");

    clock_source.map(|s| s.contains("tsc")).unwrap_or(false)
}

/// Returns (1) cycles per second, (2) last monotonic time and (3) associated tsc.
fn cycles_per_sec() -> (u64, Instant, u64) {
    let mut cycles_per_sec;
    let mut last_monotonic;
    let mut last_tsc;
    let mut old_cycles = 0.0;

    loop {
        let (t1, tsc1) = monotonic_with_tsc();
        loop {
            let (t2, tsc2) = monotonic_with_tsc();
            last_monotonic = t2;
            last_tsc = tsc2;
            let elapsed_nanos = (t2 - t1).as_nanos();
            if elapsed_nanos > 10_000_000 {
                cycles_per_sec = (tsc2 - tsc1) as f64 * 1_000_000_000.0 / elapsed_nanos as f64;
                break;
            }
        }
        let delta = f64::abs(cycles_per_sec - old_cycles);
        if delta / cycles_per_sec < 0.00001 {
            break;
        }
        old_cycles = cycles_per_sec;
    }

    (cycles_per_sec.round() as u64, last_monotonic, last_tsc)
}

/// Try to get tsc and monotonic time at the same time. Due to
/// get interrupted in half way may happen, they aren't guaranteed
/// to represent the same instant.
fn monotonic_with_tsc() -> (Instant, u64) {
    (Instant::now(), tsc())
}

#[inline]
fn tsc() -> u64 {
    #[cfg(target_arch = "aarch64")]
    unsafe fn _rdtsc() -> u64 {
        use std::arch::asm;
        let mut v: u64;
        asm!("mrs {v}, CNTVCT_EL0", v = out(reg) v);
        v
    }
    #[cfg(target_arch = "x86")]
    use core::arch::x86::_rdtsc;
    #[cfg(target_arch = "x86_64")]
    use core::arch::x86_64::_rdtsc;

    unsafe { _rdtsc() }
}

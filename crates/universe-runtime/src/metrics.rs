//! Measured process cost. Instruments, never policy.
//!
//! Claims like "a dormant construct costs nothing" and "the files are never read
//! again after boot" are only worth the measurement behind them. This module
//! asks the OS for the two counters that settle both:
//!
//!   * **CPU time** consumed by this process (user + kernel). A loop that is
//!     genuinely BLOCKED on its wake queue burns no CPU; a loop that polls does.
//!   * **I/O operation counts** issued by this process. A resident Universe that
//!     truly holds its state in memory issues zero read operations after boot.
//!
//! Every reading is wrapped in [`Epistemic`]: on a platform where the counter is
//! not available the answer is `NotMeasured`, never a fabricated zero. A failed
//! syscall is `MeasurementFailed`, never silently nominal.

use std::time::Duration;

use universe_core::Epistemic;

/// Process-wide I/O operation counters as the OS reports them.
///
/// These count operations issued by the whole process — every thread, every
/// file, plus console and device I/O. That makes them a CONSERVATIVE instrument
/// for "did the loop read the store": a zero delta is decisive; a non-zero delta
/// must be attributed before it means anything.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IoCounts {
    pub read_ops: u64,
    pub write_ops: u64,
    pub other_ops: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

impl IoCounts {
    /// `self - earlier`, saturating. The delta across a measured window.
    pub fn since(&self, earlier: &IoCounts) -> IoCounts {
        IoCounts {
            read_ops: self.read_ops.saturating_sub(earlier.read_ops),
            write_ops: self.write_ops.saturating_sub(earlier.write_ops),
            other_ops: self.other_ops.saturating_sub(earlier.other_ops),
            read_bytes: self.read_bytes.saturating_sub(earlier.read_bytes),
            write_bytes: self.write_bytes.saturating_sub(earlier.write_bytes),
        }
    }
}

#[cfg(windows)]
mod sys {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Filetime {
        pub low: u32,
        pub high: u32,
    }

    impl Filetime {
        /// FILETIME is a count of 100-nanosecond intervals.
        pub fn nanos(&self) -> u64 {
            ((u64::from(self.high) << 32) | u64::from(self.low)).saturating_mul(100)
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct IoCountersRaw {
        pub read_ops: u64,
        pub write_ops: u64,
        pub other_ops: u64,
        pub read_bytes: u64,
        pub write_bytes: u64,
        pub other_bytes: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentProcess() -> isize;
        pub fn GetProcessTimes(
            process: isize,
            creation: *mut Filetime,
            exit: *mut Filetime,
            kernel: *mut Filetime,
            user: *mut Filetime,
        ) -> i32;
        pub fn GetProcessIoCounters(process: isize, counters: *mut IoCountersRaw) -> i32;
    }
}

/// CPU time (user + kernel) this process has consumed so far.
#[cfg(windows)]
pub fn process_cpu_time() -> Epistemic<Duration> {
    let mut creation = sys::Filetime::default();
    let mut exit = sys::Filetime::default();
    let mut kernel = sys::Filetime::default();
    let mut user = sys::Filetime::default();
    // SAFETY: four out-parameters of the exact declared layout, and a pseudo
    // handle that needs no close.
    let ok = unsafe {
        sys::GetProcessTimes(
            sys::GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Epistemic::MeasurementFailed {
            reason: "GetProcessTimes failed".into(),
        };
    }
    Epistemic::Measured(Duration::from_nanos(
        kernel.nanos().saturating_add(user.nanos()),
    ))
}

/// I/O operations this process has issued so far.
#[cfg(windows)]
pub fn process_io() -> Epistemic<IoCounts> {
    let mut raw = sys::IoCountersRaw::default();
    // SAFETY: one out-parameter of the exact declared layout, pseudo handle.
    let ok = unsafe { sys::GetProcessIoCounters(sys::GetCurrentProcess(), &mut raw) };
    if ok == 0 {
        return Epistemic::MeasurementFailed {
            reason: "GetProcessIoCounters failed".into(),
        };
    }
    Epistemic::Measured(IoCounts {
        read_ops: raw.read_ops,
        write_ops: raw.write_ops,
        other_ops: raw.other_ops,
        read_bytes: raw.read_bytes,
        write_bytes: raw.write_bytes,
    })
}

/// No portable counter is wired for this platform: the honest answer is that the
/// cost was not measured here, not that it was zero.
#[cfg(not(windows))]
pub fn process_cpu_time() -> Epistemic<Duration> {
    Epistemic::NotMeasured
}

#[cfg(not(windows))]
pub fn process_io() -> Epistemic<IoCounts> {
    Epistemic::NotMeasured
}

/// The CPU consumed between two readings, or the epistemic reason it is unknown.
pub fn cpu_delta(before: &Epistemic<Duration>, after: &Epistemic<Duration>) -> Epistemic<Duration> {
    match (before, after) {
        (Epistemic::Measured(a), Epistemic::Measured(b)) => {
            Epistemic::Measured(b.saturating_sub(*a))
        }
        (Epistemic::MeasurementFailed { reason }, _) | (_, Epistemic::MeasurementFailed { reason }) => {
            Epistemic::MeasurementFailed {
                reason: reason.clone(),
            }
        }
        _ => Epistemic::NotMeasured,
    }
}

/// The I/O issued between two readings, or the epistemic reason it is unknown.
pub fn io_delta(before: &Epistemic<IoCounts>, after: &Epistemic<IoCounts>) -> Epistemic<IoCounts> {
    match (before, after) {
        (Epistemic::Measured(a), Epistemic::Measured(b)) => Epistemic::Measured(b.since(a)),
        (Epistemic::MeasurementFailed { reason }, _) | (_, Epistemic::MeasurementFailed { reason }) => {
            Epistemic::MeasurementFailed {
                reason: reason.clone(),
            }
        }
        _ => Epistemic::NotMeasured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instruments answer, and they answer monotonically. A counter that
    /// went backwards would make every delta below meaningless.
    #[test]
    fn counters_are_readable_and_monotonic() {
        let cpu_a = process_cpu_time();
        let io_a = process_io();
        // Burn a little CPU so the second reading cannot be trivially equal.
        let mut acc = 0u64;
        for i in 0..2_000_000u64 {
            acc = acc.wrapping_add(i);
        }
        assert_ne!(acc, 1);
        let cpu_b = process_cpu_time();
        let io_b = process_io();

        if let (Epistemic::Measured(a), Epistemic::Measured(b)) = (&cpu_a, &cpu_b) {
            assert!(b >= a, "process CPU time went backwards");
        }
        if let (Epistemic::Measured(a), Epistemic::Measured(b)) = (&io_a, &io_b) {
            assert!(b.read_ops >= a.read_ops, "read op counter went backwards");
            assert!(b.write_ops >= a.write_ops, "write op counter went backwards");
        }
    }
}

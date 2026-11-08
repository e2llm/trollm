//! Raw same-device read through the exact RPA-owned C ABI. This does not grant
//! authority or establish a future validity/rate interval for a native send.
use anyhow::{ensure, Result};
use serde::Serialize;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct KernelObservation {
    pub state: i32,
    pub status: i32,
    pub synchronized: i32,
    pub syscall_error: i32,
    pub max_error_us: i64,
    pub estimated_error_us: i64,
}
const _: () = assert!(std::mem::size_of::<KernelObservation>() == 32);
const _: () = assert!(std::mem::offset_of!(KernelObservation, state) == 0);
const _: () = assert!(std::mem::offset_of!(KernelObservation, status) == 4);
const _: () = assert!(std::mem::offset_of!(KernelObservation, synchronized) == 8);
const _: () = assert!(std::mem::offset_of!(KernelObservation, syscall_error) == 12);
const _: () = assert!(std::mem::offset_of!(KernelObservation, max_error_us) == 16);
const _: () = assert!(std::mem::offset_of!(KernelObservation, estimated_error_us) == 24);

#[cfg(any(target_os = "linux", target_os = "android"))]
extern "C" {
    fn moray_read_kernel_clock_v1(out: *mut KernelObservation) -> i32;
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct KernelRead {
    /// Original C return code and fields remain distinct, including failures.
    pub return_code: i32,
    pub observation: KernelObservation,
}
impl KernelRead {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn read() -> Result<Self> {
        let mut observation = KernelObservation::default();
        // Fixed owned output buffer matching the pinned header. No caller JSON,
        // environment flag, subprocess, timestamp or clock adjustment is used.
        let return_code = unsafe { moray_read_kernel_clock_v1(&mut observation) };
        ensure!(
            return_code >= 0,
            "kernel reader ABI returned an invalid code"
        );
        Ok(Self {
            return_code,
            observation,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn read() -> Result<Self> {
        anyhow::bail!("same-device kernel clock reader unsupported")
    }

    /// Apply only an explicitly selected error ceiling to this successful raw
    /// read. No timestamp/lease or host-to-device mapping is manufactured.
    pub fn checked_error_us(&self, selected_max_error_us: u64) -> Result<u64> {
        let raw = &self.observation;
        ensure!(
            self.return_code == 0 && raw.syscall_error == 0,
            "same-device kernel clock syscall failed: return={}, errno={}",
            self.return_code,
            raw.syscall_error
        );
        ensure!(
            raw.state == 0 && raw.synchronized == 1 && raw.status & (0x0040 | 0x1000) == 0,
            "same-device kernel clock synchronization unavailable"
        );
        ensure!(
            raw.max_error_us >= 0 && raw.estimated_error_us >= 0,
            "same-device kernel clock error unavailable"
        );
        let error = raw.max_error_us.max(raw.estimated_error_us) as u64;
        ensure!(
            selected_max_error_us > 0 && error <= selected_max_error_us,
            "same-device kernel clock exceeds selected error bound"
        );
        Ok(error)
    }
}

/// One same-process/device sample. These are native-produced measurements, not
/// host clock evidence, a serialized authority flag or a future validity lease.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceClockSample {
    pub boot_id: String,
    pub realtime_started_ns: u64,
    pub realtime_finished_ns: u64,
    pub boottime_started_ns: u64,
    pub boottime_finished_ns: u64,
    pub kernel_before: KernelRead,
    pub kernel_after: KernelRead,
    pub realtime_resolution_ns: u64,
    pub boottime_resolution_ns: u64,
    pub conservative_error_ns: u64,
    pub utc_upper_ms: u64,
}

/// Installed by native composition with an explicit resolved policy ceiling.
/// Every call samples the actual device again; no elapsed-time extrapolation,
/// cached-host offset, oscillator assumption or synchronization lease is used.
pub struct SameDeviceClock {
    boot_id: String,
    selected_max_error_us: u64,
    invalid: std::sync::atomic::AtomicBool,
    last_sample: std::sync::Mutex<Option<DeviceClockSample>>,
}
impl SameDeviceClock {
    pub fn new(selected_max_error_us: u64) -> Result<Self> {
        ensure!(selected_max_error_us > 0, "device clock policy unavailable");
        // Installation only. A live process cannot survive reboot into a new
        // kernel; no filesystem read is hidden in the final local sampler.
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        let boot_id = boot_id.trim();
        ensure!(
            boot_id.len() == 36
                && boot_id.bytes().enumerate().all(|(i, b)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        b == b'-'
                    } else {
                        b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                    }
                }),
            "device boot identity unavailable"
        );
        Ok(Self {
            boot_id: boot_id.into(),
            selected_max_error_us,
            invalid: false.into(),
            last_sample: std::sync::Mutex::new(None),
        })
    }

    pub fn sample(&self) -> Result<DeviceClockSample> {
        self.sample_with(acquire_device_bracket)
    }

    fn sample_with(
        &self,
        acquire: impl FnOnce() -> Result<AcquiredBracket>,
    ) -> Result<DeviceClockSample> {
        use std::sync::atomic::Ordering;
        ensure!(
            !self.invalid.load(Ordering::SeqCst),
            "device clock previously fenced"
        );
        let result = (|| {
            let (
                real_start,
                real_end,
                boot_start,
                boot_end,
                before,
                after,
                real_resolution,
                boot_resolution,
            ) = acquire()?;
            ensure!(
                real_resolution > 0 && boot_resolution > 0,
                "device clock resolution unavailable"
            );
            let quantization = real_resolution
                .checked_add(boot_resolution)
                .and_then(|n| n.checked_mul(2))
                .ok_or_else(|| anyhow::anyhow!("device clock quantization overflow"))?;
            let error_us = before
                .checked_error_us(self.selected_max_error_us)?
                .max(after.checked_error_us(self.selected_max_error_us)?);
            let real_span = real_end
                .checked_sub(real_start)
                .ok_or_else(|| anyhow::anyhow!("device realtime moved backwards"))?;
            let boot_span = boot_end
                .checked_sub(boot_start)
                .ok_or_else(|| anyhow::anyhow!("device boottime moved backwards"))?;
            let kernel_error_ns = error_us
                .checked_mul(1000)
                .ok_or_else(|| anyhow::anyhow!("device clock error overflow"))?;
            // Realtime brackets enclose the boottime brackets. Charge the FULL
            // observed acquisition duration and both measured clock resolutions at both endpoints.
            // A detected disagreement beyond the measured error fails closed.
            ensure!(
                real_span.abs_diff(boot_span)
                    <= kernel_error_ns
                        .checked_mul(2)
                        .and_then(|n| n.checked_add(quantization))
                        .ok_or_else(|| anyhow::anyhow!("device discontinuity bound overflow"))?,
                "device clock discontinuity"
            );
            let uncertainty = kernel_error_ns
                .checked_add(real_span)
                .and_then(|n| n.checked_add(boot_span))
                .and_then(|n| n.checked_add(quantization))
                .ok_or_else(|| anyhow::anyhow!("device sample uncertainty overflow"))?;
            let ceiling = self
                .selected_max_error_us
                .checked_mul(1000)
                .ok_or_else(|| anyhow::anyhow!("device clock ceiling overflow"))?;
            ensure!(
                uncertainty <= ceiling,
                "device sample exceeds selected error ceiling"
            );
            let upper = real_end
                .checked_add(uncertainty)
                .and_then(|n| n.checked_add(999_999))
                .ok_or_else(|| anyhow::anyhow!("device UTC upper bound overflow"))?
                / 1_000_000;
            let sample = DeviceClockSample {
                boot_id: self.boot_id.clone(),
                realtime_started_ns: real_start,
                realtime_finished_ns: real_end,
                boottime_started_ns: boot_start,
                boottime_finished_ns: boot_end,
                kernel_before: before,
                kernel_after: after,
                realtime_resolution_ns: real_resolution,
                boottime_resolution_ns: boot_resolution,
                conservative_error_ns: uncertainty,
                utc_upper_ms: upper,
            };
            let mut last = self
                .last_sample
                .try_lock()
                .map_err(|_| anyhow::anyhow!("device clock sample state pending or unavailable"))?;
            if let Some(previous) = last.as_ref() {
                ensure!(
                    sample.boottime_started_ns >= previous.boottime_finished_ns
                        && sample.realtime_started_ns >= previous.realtime_finished_ns,
                    "device clock continuity lost"
                );
                ensure!(
                    sample.realtime_resolution_ns == previous.realtime_resolution_ns
                        && sample.boottime_resolution_ns == previous.boottime_resolution_ns,
                    "device clock resolution changed"
                );
                let elapsed_real = sample.realtime_started_ns - previous.realtime_finished_ns;
                let elapsed_boot = sample.boottime_started_ns - previous.boottime_finished_ns;
                let endpoint_error = sample
                    .conservative_error_ns
                    .checked_add(previous.conservative_error_ns)
                    .ok_or_else(|| anyhow::anyhow!("device continuity error overflow"))?;
                ensure!(
                    elapsed_real.abs_diff(elapsed_boot) <= endpoint_error,
                    "device observed clock-domain discontinuity"
                );
            }
            ensure!(
                !self.invalid.load(Ordering::SeqCst),
                "device clock concurrently fenced"
            );
            *last = Some(sample.clone());
            Ok(sample)
        })();
        if result.is_err() {
            self.invalid.store(true, Ordering::SeqCst);
        }
        result
    }

    /// Retrospective native sample only. Never reusable as send authority.
    pub fn last_sample(&self) -> Result<Option<DeviceClockSample>> {
        Ok(self
            .last_sample
            .lock()
            .map_err(|_| anyhow::anyhow!("device clock sample lock failed"))?
            .clone())
    }
}

type AcquiredBracket = (u64, u64, u64, u64, KernelRead, KernelRead, u64, u64);

#[cfg(any(target_os = "linux", target_os = "android"))]
fn acquire_device_bracket() -> Result<AcquiredBracket> {
    let realtime_resolution = clock_stamp(libc::CLOCK_REALTIME, true)?;
    let boottime_resolution = clock_stamp(libc::CLOCK_BOOTTIME, true)?;
    let realtime_start = clock_stamp(libc::CLOCK_REALTIME, false)?;
    let boottime_start = clock_stamp(libc::CLOCK_BOOTTIME, false)?;
    let before = KernelRead::read()?;
    let after = KernelRead::read()?;
    let boottime_end = clock_stamp(libc::CLOCK_BOOTTIME, false)?;
    let realtime_end = clock_stamp(libc::CLOCK_REALTIME, false)?;
    Ok((
        realtime_start,
        realtime_end,
        boottime_start,
        boottime_end,
        before,
        after,
        realtime_resolution,
        boottime_resolution,
    ))
}
#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn acquire_device_bracket() -> Result<AcquiredBracket> {
    anyhow::bail!("same-device clock brackets unsupported")
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn clock_stamp(id: libc::clockid_t, resolution: bool) -> Result<u64> {
    let mut stamp = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // Owned output and read-only clock syscalls, never an adjustment.
    let code = unsafe {
        if resolution {
            libc::clock_getres(id, &mut stamp)
        } else {
            libc::clock_gettime(id, &mut stamp)
        }
    };
    ensure!(
        code == 0,
        "device clock read failed: {}",
        std::io::Error::last_os_error()
    );
    ensure!(
        stamp.tv_sec >= 0 && (0..1_000_000_000).contains(&stamp.tv_nsec),
        "device clock timestamp unavailable"
    );
    (stamp.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|n| n.checked_add(stamp.tv_nsec as u64))
        .ok_or_else(|| anyhow::anyhow!("device clock timestamp overflow"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn controlled_clock() -> SameDeviceClock {
        SameDeviceClock {
            boot_id: "00000000-0000-0000-0000-000000000001".into(),
            selected_max_error_us: 100,
            invalid: false.into(),
            last_sample: std::sync::Mutex::new(None),
        }
    }
    fn raw_clock() -> KernelRead {
        KernelRead {
            return_code: 0,
            observation: KernelObservation {
                synchronized: 1,
                max_error_us: 10,
                ..Default::default()
            },
        }
    }
    #[test]
    fn final_device_sample_refuses_pending_state_without_waiting() {
        let clock = std::sync::Arc::new(controlled_clock());
        let held = clock.last_sample.lock().unwrap();
        let worker_clock = clock.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = worker_clock.sample_with(|| {
                Ok((
                    1_000_000_000,
                    1_000_001_000,
                    10_000,
                    10_800,
                    raw_clock(),
                    raw_clock(),
                    1,
                    1,
                ))
            });
            tx.send(result).unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(1));
        // Release and join even on a regression, rather than hanging the job.
        drop(held);
        worker.join().unwrap();
        let error = result
            .expect("final sampling must not wait on state")
            .unwrap_err();
        assert!(error.to_string().contains("sample state pending"));
        assert!(clock
            .sample_with(|| unreachable!("pending final sample fences clock"))
            .is_err());
    }
    #[test]
    fn fresh_device_brackets_charge_both_durations_and_latch_every_loss() {
        let clock = controlled_clock();
        let sample = clock
            .sample_with(|| {
                Ok((
                    1_000_000_000,
                    1_000_001_000,
                    10_000,
                    10_800,
                    raw_clock(),
                    raw_clock(),
                    1,
                    1,
                ))
            })
            .unwrap();
        assert_eq!(sample.conservative_error_ns, 11_804);
        assert_eq!(sample.utc_upper_ms, 1001);
        assert_eq!(
            clock.last_sample().unwrap().unwrap().realtime_finished_ns,
            1_000_001_000
        );
        assert!(clock
            .sample_with(|| Ok((
                1_000_000_500,
                1_000_002_000,
                11_000,
                12_000,
                raw_clock(),
                raw_clock(),
                1,
                1
            )))
            .is_err());
        assert!(clock
            .sample_with(|| Ok((
                2_000_000_000,
                2_000_001_000,
                20_000,
                20_800,
                raw_clock(),
                raw_clock(),
                1,
                1
            )))
            .is_err());
        for mode in 0..7 {
            let clock = controlled_clock();
            assert!(clock
                .sample_with(|| {
                    let mut before = raw_clock();
                    let mut after = raw_clock();
                    match mode {
                        0 => before.return_code = libc::EIO,
                        1 => after.observation.synchronized = 0,
                        2 => return Ok((10, 9, 1, 2, before, after, 1, 1)),
                        3 => return Ok((10, 11, 2, 1, before, after, 1, 1)),
                        4 => return Ok((10, 1_000_000, 1, 2, before, after, 1, 1)),
                        5 => return Ok((u64::MAX - 1, u64::MAX, 1, 2, before, after, 1, 1)),
                        _ => return Err(anyhow::anyhow!("controlled clock syscall loss")),
                    }
                    Ok((1_000_000, 1_000_100, 10, 100, before, after, 1, 1))
                })
                .is_err());
            assert!(clock
                .sample_with(|| unreachable!("latched loss must refuse before another acquisition"))
                .is_err());
        }
    }
    #[test]
    fn device_samples_refuse_between_sample_jump_precision_drift_and_unbounded_resolution() {
        for mode in 0..4 {
            let clock = controlled_clock();
            clock
                .sample_with(|| {
                    Ok((
                        1_000_000_000,
                        1_000_001_000,
                        10_000,
                        10_800,
                        raw_clock(),
                        raw_clock(),
                        1,
                        1,
                    ))
                })
                .unwrap();
            let result = clock.sample_with(|| {
                let start = if mode == 0 {
                    2_000_000_000
                } else {
                    1_000_002_000
                };
                let resolution = match mode {
                    1 => 2,
                    2 => 0,
                    3 => 100_000,
                    _ => 1,
                };
                Ok((
                    start,
                    start + 1_000,
                    11_800,
                    12_600,
                    raw_clock(),
                    raw_clock(),
                    resolution,
                    1,
                ))
            });
            assert!(result.is_err());
            assert!(clock
                .sample_with(|| unreachable!("no resampling after clock loss"))
                .is_err());
        }
    }
    #[test]
    fn pinned_source_and_header_match_the_acquired_abi() {
        for (bytes, expected) in [
            (
                include_bytes!("../vendor/rpa-kernel-clock/kernel-clock-reader.h").as_slice(),
                "6894f72bf259c0026df95a43294be8fc9eb219713ad347b4a9cfa81b83376aca",
            ),
            (
                include_bytes!("../vendor/rpa-kernel-clock/kernel-clock-probe.c").as_slice(),
                "d18a9d227ce11232371d42526836bfc20726d325cba6daf7dc12cd32be5894a1",
            ),
        ] {
            assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected);
        }
    }

    #[test]
    fn selected_policy_refuses_error_unsynchronized_and_absent_bounds() {
        let good = KernelRead {
            return_code: 0,
            observation: KernelObservation {
                state: 0,
                status: 0,
                synchronized: 1,
                syscall_error: 0,
                max_error_us: 100,
                estimated_error_us: 50,
            },
        };
        assert_eq!(good.checked_error_us(100).unwrap(), 100);
        assert!(good.checked_error_us(99).is_err());
        assert!(good.checked_error_us(0).is_err());
        for bad in [
            KernelRead {
                return_code: libc::EIO,
                observation: KernelObservation {
                    state: -1,
                    syscall_error: libc::EIO,
                    ..Default::default()
                },
            },
            KernelRead {
                observation: KernelObservation {
                    status: 0x0040,
                    ..good.observation
                },
                ..good
            },
            KernelRead {
                observation: KernelObservation {
                    status: 0x1000,
                    ..good.observation
                },
                ..good
            },
            KernelRead {
                observation: KernelObservation {
                    state: 5,
                    ..good.observation
                },
                ..good
            },
            KernelRead {
                observation: KernelObservation {
                    synchronized: 0,
                    ..good.observation
                },
                ..good
            },
            KernelRead {
                observation: KernelObservation {
                    max_error_us: -1,
                    ..good.observation
                },
                ..good
            },
            KernelRead {
                observation: KernelObservation {
                    estimated_error_us: -1,
                    ..good.observation
                },
                ..good
            },
        ] {
            assert!(bad.checked_error_us(1000).is_err());
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn actual_linked_reader_preserves_raw_fields_without_assuming_host_sync() {
        assert_eq!(
            unsafe { moray_read_kernel_clock_v1(std::ptr::null_mut()) },
            libc::EINVAL
        );
        let actual = KernelRead::read().unwrap();
        if actual.return_code != 0 {
            assert_eq!(actual.observation.state, -1);
            assert_eq!(actual.observation.syscall_error, actual.return_code);
            assert!(actual.checked_error_us(1000).is_err());
        } else {
            assert_eq!(actual.observation.syscall_error, 0);
            assert!(matches!(actual.observation.synchronized, 0 | 1));
        }
    }
}

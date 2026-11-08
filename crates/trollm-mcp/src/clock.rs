//! Native final-write age mapping. UTC assurance is an installed provider input,
//! not established by deserializing a timestamp/hash. Linux/Android BOOTTIME
//! includes suspend; std Instant alone must not renew validity across sleep.
use anyhow::{ensure, Context, Result};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootSample {
    boot_id: String,
    nanos: u64,
}
impl BootSample {
    pub fn boot_id(&self) -> &str {
        &self.boot_id
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn read() -> Result<Self> {
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
        let boot_id = boot_id.trim().to_owned();
        ensure!(
            boot_id.len() == 36 && boot_id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'),
            "native boot identity unavailable"
        );
        let mut clock = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // A fixed kernel output buffer; no caller-owned pointer is accepted.
        ensure!(
            unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut clock) } == 0,
            "native suspend-aware clock unavailable"
        );
        ensure!(
            clock.tv_sec >= 0 && (0..1_000_000_000).contains(&clock.tv_nsec),
            "native clock invalid"
        );
        let nanos = (clock.tv_sec as u64)
            .checked_mul(1_000_000_000)
            .and_then(|n| n.checked_add(clock.tv_nsec as u64))
            .context("native clock overflow")?;
        Ok(Self { boot_id, nanos })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn read() -> Result<Self> {
        anyhow::bail!("native suspend-aware clock unsupported")
    }
}

/// Original endpoint custody. MONOTONIC reads enclose the BOOTTIME syscall;
/// neither endpoint is reconstructed from a later continuity witness.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockEndpoint {
    pub boot: BootSample,
    pub monotonic_before_ns: u64,
    pub monotonic_after_ns: u64,
}
impl ClockEndpoint {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn read() -> Result<Self> {
        fn monotonic() -> Result<u64> {
            let mut stamp = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            ensure!(
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut stamp) } == 0,
                "endpoint MONOTONIC unavailable"
            );
            ensure!(
                stamp.tv_sec >= 0 && (0..1_000_000_000).contains(&stamp.tv_nsec),
                "endpoint MONOTONIC invalid"
            );
            (stamp.tv_sec as u64)
                .checked_mul(1_000_000_000)
                .and_then(|n| n.checked_add(stamp.tv_nsec as u64))
                .context("endpoint MONOTONIC overflow")
        }
        let monotonic_before_ns = monotonic()?;
        let boot = BootSample::read()?;
        let monotonic_after_ns = monotonic()?;
        ensure!(
            monotonic_after_ns >= monotonic_before_ns,
            "endpoint MONOTONIC regressed"
        );
        Ok(Self {
            boot,
            monotonic_before_ns,
            monotonic_after_ns,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn read() -> Result<Self> {
        anyhow::bail!("native endpoint clock unsupported")
    }
}

/// Local cross-domain witness. The boot ID is captured once during installation;
/// this process cannot survive a reboot. Final reads perform only clock syscalls.
#[derive(Clone)]
pub struct ElapsedWitness {
    pub boot: BootSample,
    real_boot_lower: i128,
    real_boot_upper: i128,
    boot_mono_lower: i128,
    boot_mono_upper: i128,
}
impl ElapsedWitness {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn read(boot_id: &str) -> Result<Self> {
        fn stamp(domain: libc::clockid_t, resolution: bool) -> Result<u64> {
            let mut value = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let result = unsafe {
                if resolution {
                    libc::clock_getres(domain, &mut value)
                } else {
                    libc::clock_gettime(domain, &mut value)
                }
            };
            ensure!(
                result == 0 && value.tv_sec >= 0 && (0..1_000_000_000).contains(&value.tv_nsec),
                "calibrated clock domain unavailable"
            );
            (value.tv_sec as u64)
                .checked_mul(1_000_000_000)
                .and_then(|n| n.checked_add(value.tv_nsec as u64))
                .context("clock stamp overflow")
        }
        let resolution = [
            libc::CLOCK_BOOTTIME,
            libc::CLOCK_MONOTONIC,
            libc::CLOCK_REALTIME,
        ]
        .into_iter()
        .try_fold(0u64, |sum, clock| -> Result<u64> {
            sum.checked_add(stamp(clock, true)?)
                .context("clock resolution overflow")
        })?;
        ensure!(resolution > 0, "clock resolution unavailable");
        let m1 = stamp(libc::CLOCK_MONOTONIC, false)?;
        let b1 = stamp(libc::CLOCK_BOOTTIME, false)?;
        let real = stamp(libc::CLOCK_REALTIME, false)?;
        let b2 = stamp(libc::CLOCK_BOOTTIME, false)?;
        let m2 = stamp(libc::CLOCK_MONOTONIC, false)?;
        ensure!(b2 >= b1 && m2 >= m1, "clock witness moved backwards");
        let q = i128::from(resolution);
        Ok(Self {
            boot: BootSample {
                boot_id: boot_id.into(),
                nanos: b2,
            },
            real_boot_lower: i128::from(real) - i128::from(b2) - q,
            real_boot_upper: i128::from(real) - i128::from(b1) + q,
            boot_mono_lower: i128::from(b1) - i128::from(m2) - q,
            boot_mono_upper: i128::from(b2) - i128::from(m1) + q,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    pub fn read(_: &str) -> Result<Self> {
        anyhow::bail!("calibrated clock platform unavailable")
    }

    /// Intersect with the original witness, not a rolling tolerance that could
    /// launder cumulative drift. UTC comes only from the authenticated anchor.
    pub fn intersect(&self, current: &Self) -> Result<Self> {
        ensure!(
            self.boot.boot_id == current.boot.boot_id && current.boot.nanos >= self.boot.nanos,
            "calibrated clock boot or monotonic continuity lost"
        );
        let real_boot_lower = self.real_boot_lower.max(current.real_boot_lower);
        let real_boot_upper = self.real_boot_upper.min(current.real_boot_upper);
        let boot_mono_lower = self.boot_mono_lower.max(current.boot_mono_lower);
        let boot_mono_upper = self.boot_mono_upper.min(current.boot_mono_upper);
        ensure!(
            real_boot_lower <= real_boot_upper,
            "device wall clock stepped"
        );
        ensure!(
            boot_mono_lower <= boot_mono_upper,
            "device suspend continuity lost"
        );
        Ok(Self {
            boot: current.boot.clone(),
            real_boot_lower,
            real_boot_upper,
            boot_mono_lower,
            boot_mono_upper,
        })
    }
}

/// Selected platform engineering bound, not measured/approved by parsing JSON.
/// A lower rate n/d means at least n guest nanoseconds per d trusted elapsed
/// nanoseconds for the named lifecycle. Callers must refuse unsupported profiles.
#[derive(Clone, Copy)]
pub struct ElapsedRate {
    numerator: u64,
    denominator: u64,
}
impl ElapsedRate {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self> {
        ensure!(
            numerator > 0 && denominator > 0 && numerator <= denominator,
            "elapsed-clock lower rate unavailable"
        );
        Ok(Self {
            numerator,
            denominator,
        })
    }
    fn upper_elapsed_ms(&self, guest_ns: u64) -> Result<u64> {
        // One ceiling after rational conversion, not truncated ppm/floating point.
        let n = u128::from(guest_ns) * u128::from(self.denominator);
        let d = u128::from(self.numerator) * 1_000_000;
        u64::try_from(n / d + u128::from(n % d != 0))
            .context("rate-adjusted elapsed bound overflow")
    }
}

/// Runtime-only anchor, never restored from serialized state. The installed
/// native verifier must authenticate the UTC upper bound, its error and validity
/// and bind the sample to this request/authority before constructing this anchor.
/// native_mapping_valid_until_ms is independently justified forward mapping
/// validity, never a retrospective witness coverage end. A kernel measurement
/// validUntil that ends at its last observation cannot supply this argument.
/// Without a supported native error/rate bound through send, do not construct.
#[derive(Debug, Clone, Copy)]
pub enum ClockMappingFailure {
    Discontinuity,
    Stale,
    Expired,
}
impl std::fmt::Display for ClockMappingFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Discontinuity => "SELECTED_CLOCK_DISCONTINUITY",
            Self::Stale => "SELECTED_CLOCK_STALE",
            Self::Expired => "SELECTED_CLOCK_DEADLINE",
        })
    }
}
impl std::error::Error for ClockMappingFailure {}

pub struct VerifiedUtcAnchor {
    received: BootSample,
    upper_at_receive_ms: u64,
    native_mapping_valid_until_ms: u64,
    rate: ElapsedRate,
    maximum_elapsed_ms: u64,
    acquisition_elapsed_ms: u64,
}
impl VerifiedUtcAnchor {
    pub fn from_verified_sample(
        sent: BootSample,
        received: BootSample,
        verified_sample_utc_ms: u64,
        verified_error_ms: u64,
        native_mapping_valid_until_ms: u64,
    ) -> Result<Self> {
        Self::from_verified_sample_with_rate(
            sent,
            received,
            verified_sample_utc_ms,
            verified_error_ms,
            native_mapping_valid_until_ms,
            ElapsedRate::new(1, 1)?,
            u64::MAX,
        )
    }
    /// Only installed composition for a supported named platform may call this.
    /// A declared numeric rate/profile never establishes its own qualification.
    pub fn from_verified_sample_with_rate(
        sent: BootSample,
        received: BootSample,
        verified_sample_utc_ms: u64,
        verified_error_ms: u64,
        native_mapping_valid_until_ms: u64,
        rate: ElapsedRate,
        maximum_elapsed_ms: u64,
    ) -> Result<Self> {
        ensure!(maximum_elapsed_ms > 0, "calibration lifetime unavailable");
        ensure!(
            sent.boot_id == received.boot_id && received.nanos >= sent.nanos,
            "clock request bracket discontinuity"
        );
        let acquisition_elapsed_ms = rate.upper_elapsed_ms(received.nanos - sent.nanos)?;
        ensure!(
            acquisition_elapsed_ms <= maximum_elapsed_ms,
            "calibration acquisition aged out"
        );
        // The verifier sample must have been acquired within the authenticated
        // request bracket. Charging full RTT is conservative, never half-RTT.
        let upper = verified_sample_utc_ms
            .checked_add(verified_error_ms)
            .and_then(|n| n.checked_add(acquisition_elapsed_ms))
            .context("verified UTC bound overflow")?;
        ensure!(
            upper < native_mapping_valid_until_ms,
            "native clock mapping expired"
        );
        Ok(Self {
            received,
            upper_at_receive_ms: upper,
            native_mapping_valid_until_ms,
            rate,
            maximum_elapsed_ms,
            acquisition_elapsed_ms,
        })
    }
    /// Conservative remaining ORIGINAL mapping-age budget, including request RTT.
    pub fn remaining_age_ms(&self, now: &BootSample) -> Result<u64> {
        if now.boot_id != self.received.boot_id || now.nanos < self.received.nanos {
            return Err(ClockMappingFailure::Discontinuity.into());
        }
        let elapsed = self
            .rate
            .upper_elapsed_ms(now.nanos - self.received.nanos)?;
        self.maximum_elapsed_ms
            .checked_sub(
                self.acquisition_elapsed_ms
                    .checked_add(elapsed)
                    .context("native clock age overflow")?,
            )
            .ok_or_else(|| ClockMappingFailure::Stale.into())
    }
    pub fn remaining_validity_ms(&self, now: &BootSample) -> Result<u64> {
        let upper = self.upper_now_ms(now)?;
        Ok(self
            .remaining_age_ms(now)?
            .min(self.native_mapping_valid_until_ms - upper))
    }
    pub fn upper_now_ms(&self, now: &BootSample) -> Result<u64> {
        self.remaining_age_ms(now)?;
        let elapsed = self
            .rate
            .upper_elapsed_ms(now.nanos - self.received.nanos)?;
        let upper = self
            .upper_at_receive_ms
            .checked_add(elapsed)
            .context("native UTC upper bound overflow")?;
        if upper >= self.native_mapping_valid_until_ms {
            return Err(ClockMappingFailure::Expired.into());
        }
        Ok(upper)
    }
    /// Final-send check consumes original validity; no preparation reserve is
    /// reapplied, offset inferred, wall clock trusted or cutoff extended.
    pub fn check_before_send(&self, original_effective_expiry_ms: u64) -> Result<()> {
        ensure!(
            self.upper_now_ms(&BootSample::read()?)? < original_effective_expiry_ms,
            "selected preparation expired before send"
        );
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample(nanos: u64) -> BootSample {
        BootSample {
            boot_id: "fixture-boot".into(),
            nanos,
        }
    }
    #[test]
    fn conservative_full_roundtrip_elapsed_and_discontinuity_never_renew() {
        let anchor =
            VerifiedUtcAnchor::from_verified_sample(sample(0), sample(2_000_001), 1000, 5, 2000)
                .unwrap();
        assert_eq!(anchor.upper_now_ms(&sample(2_000_001)).unwrap(), 1008);
        assert_eq!(anchor.upper_now_ms(&sample(12_000_002)).unwrap(), 1019);
        assert!(anchor.upper_now_ms(&sample(1)).is_err());
        let mut changed = sample(12_000_002);
        changed.boot_id = "restarted".into();
        assert!(anchor.upper_now_ms(&changed).is_err());
        assert!(anchor.upper_now_ms(&sample(2_000_000_000)).is_err());
        assert!(
            VerifiedUtcAnchor::from_verified_sample(sample(0), sample(10), 1000, 5, 1005).is_err()
        );
    }
    #[test]
    fn explicit_rate_charges_original_rtt_elapsed_and_independent_age_without_renewal() {
        let anchor = VerifiedUtcAnchor::from_verified_sample_with_rate(
            sample(0),
            sample(2_000_001),
            1000,
            5,
            2000,
            ElapsedRate::new(1, 2).unwrap(),
            25,
        )
        .unwrap();
        assert_eq!(anchor.upper_now_ms(&sample(2_000_001)).unwrap(), 1010);
        assert_eq!(anchor.upper_now_ms(&sample(12_000_001)).unwrap(), 1030);
        assert!(anchor.upper_now_ms(&sample(12_000_002)).is_err());
        assert!(ElapsedRate::new(0, 1).is_err());
        assert!(ElapsedRate::new(2, 1).is_err());
        assert!(ElapsedRate::new(1, 0).is_err());
        assert!(ElapsedRate::new(1, u64::MAX)
            .unwrap()
            .upper_elapsed_ms(u64::MAX)
            .is_err());
    }
    #[test]
    fn original_cross_domain_intersection_refuses_wall_steps_suspend_and_cumulative_drift() {
        let initial = ElapsedWitness {
            boot: sample(10),
            real_boot_lower: 100,
            real_boot_upper: 110,
            boot_mono_lower: 0,
            boot_mono_upper: 5,
        };
        let mut next = initial.clone();
        next.boot = sample(20);
        next.real_boot_lower = 105;
        next.real_boot_upper = 115;
        let tightened = initial.intersect(&next).unwrap();
        next.real_boot_lower = 111;
        next.real_boot_upper = 121;
        assert!(tightened.intersect(&next).is_err());
        next = initial.clone();
        next.boot = sample(20);
        next.boot_mono_lower = 6;
        next.boot_mono_upper = 8;
        assert!(initial.intersect(&next).is_err());
        next = initial.clone();
        next.boot = sample(9);
        assert!(initial.intersect(&next).is_err());
        let actual = BootSample::read().unwrap();
        let a = ElapsedWitness::read(actual.boot_id()).unwrap();
        let b = ElapsedWitness::read(actual.boot_id()).unwrap();
        a.intersect(&b).unwrap();
    }
    #[test]
    fn actual_native_clock_is_suspend_aware_and_same_boot() {
        let before = BootSample::read().unwrap();
        let after = BootSample::read().unwrap();
        assert_eq!(before.boot_id, after.boot_id);
        assert!(after.nanos >= before.nanos);
    }
}

/// Closed UTC spelling used by the selected RPA profile. libc timegm provides
/// calendar conversion; normalized fields are checked so invalid dates refuse.
#[cfg(unix)]
pub fn parse_selected_utc_ms(text: &str) -> Result<u64> {
    let bytes = text.as_bytes();
    ensure!(
        bytes.len() == 20 || bytes.len() == 24,
        "selected UTC spelling invalid"
    );
    for (position, expected) in [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')] {
        ensure!(
            bytes[position] == expected,
            "selected UTC separator invalid"
        );
    }
    ensure!(
        *bytes.last().unwrap() == b'Z',
        "selected UTC offset unsupported"
    );
    if bytes.len() == 24 {
        ensure!(bytes[19] == b'.', "selected UTC precision invalid");
    }
    let digits = |start: usize, count: usize| -> Result<i32> {
        let part = &bytes[start..start + count];
        ensure!(
            part.iter().all(u8::is_ascii_digit),
            "selected UTC digits invalid"
        );
        Ok(part.iter().fold(0i32, |n, b| n * 10 + i32::from(b - b'0')))
    };
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    let hour = digits(11, 2)?;
    let minute = digits(14, 2)?;
    let second = digits(17, 2)?;
    ensure!(
        year >= 1970
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60,
        "selected UTC range invalid"
    );
    // The initialized C structure is local, UTC conversion performs no clock write.
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    value.tm_year = year - 1900;
    value.tm_mon = month - 1;
    value.tm_mday = day;
    value.tm_hour = hour;
    value.tm_min = minute;
    value.tm_sec = second;
    let seconds = unsafe { libc::timegm(&mut value) };
    ensure!(
        seconds >= 0
            && value.tm_year == year - 1900
            && value.tm_mon == month - 1
            && value.tm_mday == day
            && value.tm_hour == hour
            && value.tm_min == minute
            && value.tm_sec == second,
        "selected UTC calendar invalid"
    );
    let fraction = if bytes.len() == 24 {
        digits(20, 3)? as u64
    } else {
        0
    };
    (seconds as u64)
        .checked_mul(1000)
        .and_then(|n| n.checked_add(fraction))
        .context("selected UTC overflow")
}

#[cfg(not(unix))]
pub fn parse_selected_utc_ms(_: &str) -> Result<u64> {
    anyhow::bail!("selected native UTC parser unsupported on this platform")
}
#[cfg(test)]
mod utc_controls {
    use super::*;
    #[test]
    fn strict_utc_calendar_precision_and_no_offset_aliases() {
        assert_eq!(parse_selected_utc_ms("1970-01-01T00:00:00Z").unwrap(), 0);
        assert_eq!(
            parse_selected_utc_ms("2024-02-29T00:00:00.007Z").unwrap(),
            parse_selected_utc_ms("2024-02-29T00:00:00Z").unwrap() + 7
        );
        for invalid in [
            "2026-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2026-11-04T24:00:00Z",
            "2026-11-04T00:00:60Z",
            "2026-11-04T00:00:00+00:00",
            "2026-11-04T00:00:00.0Z",
            "2026-11-04T00:00:00.0000Z",
            "2026-11-04t00:00:00Z",
            "2026-11-04T00:00:00Z\n",
        ] {
            assert!(parse_selected_utc_ms(invalid).is_err(), "{invalid}");
        }
    }
}

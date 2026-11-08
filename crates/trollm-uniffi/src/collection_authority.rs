//! Application publication authority and fresh same-device clock composition.
//! This is not installed by a serialized source/support flag. Run composition
//! must supply the original native run association and actual app provider.
use crate::collection::{raw_sha256, SelectionRecord};
use crate::collection_provider::InstalledSelectedEnvironment;
use crate::TokenProvider;
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use trollm_mcp::clock::parse_selected_utc_ms;
use trollm_mcp::kernel_clock::SameDeviceClock;
use trollm_mcp::preparation::PreparationContext;

struct UtcSample {
    upper_ms: u64,
    lower_ms: u64,
}
enum SelectedClock {
    Kernel(Box<SameDeviceClock>),
    Calibrated(Arc<crate::collection_clock::CalibratedClock>),
}
impl SelectedClock {
    fn acquire(&self) -> Result<()> {
        match self {
            Self::Kernel(_) => Ok(()),
            Self::Calibrated(clock) => clock.acquire(),
        }
    }
    fn sample(&self) -> Result<UtcSample> {
        match self {
            Self::Kernel(clock) => {
                let sample = clock.sample()?;
                Ok(UtcSample {
                    upper_ms: sample.utc_upper_ms,
                    lower_ms: sample
                        .realtime_started_ns
                        .checked_sub(sample.conservative_error_ns)
                        .context("authority UTC lower bound unavailable")?
                        / 1_000_000,
                })
            }
            Self::Calibrated(clock) => {
                let (upper_ms, lower_ms) = clock.sample()?;
                Ok(UtcSample { upper_ms, lower_ms })
            }
        }
    }
}

struct AcquiredAuthority {
    context_sha256: String,
    oldest_utc_lower_ms: u64,
}

impl AcquiredAuthority {
    fn validate(&self, context_sha256: &str, now_upper_ms: u64, maximum_age_ms: u64) -> Result<()> {
        ensure!(
            self.context_sha256 == context_sha256,
            "authority belongs to another native call"
        );
        validate_age(now_upper_ms, self.oldest_utc_lower_ms, maximum_age_ms)
    }
}

/// Network work happens only in acquisition. Final plaintext admission uses
/// bounded in-memory association plus a NEW actual device sample. Host/RPA
/// clock data and caller-provided timestamps cannot construct this clock.
pub struct ApplicationSelectedEnvironment {
    provider: Arc<Box<dyn TokenProvider>>,
    association_json: String,
    run_id: String,
    selection_sha256: String,
    publication_expires_ms: u64,
    maximum_authority_age_ms: u64,
    clock: SelectedClock,
    acquired: Mutex<Option<AcquiredAuthority>>,
}

impl ApplicationSelectedEnvironment {
    /// Startup authorization is required even for an empty/end-only run. It
    /// creates no call context or reusable effect preparation.
    pub fn authorize_run_start(&self) -> Result<u64> {
        crate::selected_start::boundary(self.clock.acquire(), "SELECTED_CLOCK_ACQUISITION")?;
        let before = self.sample()?;
        crate::selected_start::boundary(
            (|| -> Result<()> {
                ensure!(
                    self.provider
                        .source_authority_current(self.association_json.clone()),
                    "authenticated current source authority unavailable"
                );
                Ok(())
            })(),
            "SELECTED_SOURCE_AUTHORITY_UNAVAILABLE",
        )?;
        let after = self.sample()?;
        validate_age(
            after.upper_ms,
            oldest_lower_ms(&before)?,
            self.maximum_authority_age_ms,
        )?;
        Ok(after.upper_ms)
    }

    pub fn new(
        provider: Arc<Box<dyn TokenProvider>>,
        association: &Value,
        selection: &SelectionRecord,
        maximum_authority_age_ms: u64,
        selected_device_max_error_us: u64,
    ) -> Result<Self> {
        Self::new_with_clock(
            provider,
            association,
            selection,
            maximum_authority_age_ms,
            SelectedClock::Kernel(Box::new(SameDeviceClock::new(
                selected_device_max_error_us,
            )?)),
        )
    }
    pub(crate) fn new_calibrated(
        provider: Arc<Box<dyn TokenProvider>>,
        association: &Value,
        selection: &SelectionRecord,
        maximum_authority_age_ms: u64,
        clock: crate::collection_clock::CalibratedClock,
    ) -> Result<Self> {
        Self::new_with_clock(
            provider,
            association,
            selection,
            maximum_authority_age_ms,
            SelectedClock::Calibrated(Arc::new(clock)),
        )
    }
    fn new_with_clock(
        provider: Arc<Box<dyn TokenProvider>>,
        association: &Value,
        selection: &SelectionRecord,
        maximum_authority_age_ms: u64,
        clock: SelectedClock,
    ) -> Result<Self> {
        let source: crate::collection_members::CollectionSource =
            serde_json::from_value(association["descriptor"]["source"].clone())?;
        let selection_sha256 = raw_sha256(&selection.original_bytes);
        ensure!(
            source.schema_version == crate::collection_members::SOURCE_SCHEMA
                && matches!(
                    source.profile_id.as_str(),
                    crate::collection_members::PROFILE | crate::collection_members::MATCHED_PROFILE
                )
                && source.qualification == "unqualified"
                && source.selection_ref == selection.selection_ref
                && source.selection_raw_sha256 == selection_sha256
                && source.owner == selection.selection.owner
                && association["installation"]["installationSha256"] == source.installation_sha256
                && association["descriptor"]["identity"]["manifestSha256"]
                    == source.installation_sha256,
            "installed authority source/selection association mismatch"
        );
        let run_id = association["nativeRunId"]
            .as_str()
            .context("installed authority requires genuine selected native run")?
            .to_owned();
        crate::history::validate_run_id(&run_id)?;
        ensure!(
            maximum_authority_age_ms > 0,
            "authority age policy unavailable"
        );
        let publication_expires_ms = parse_selected_utc_ms(
            association["descriptor"]["expiresAt"]
                .as_str()
                .context("original publication expiry unavailable")?,
        )?;
        let association_json = serde_json::to_string(association)?;
        ensure!(
            association_json.len() <= 1024 * 1024,
            "authority association exceeds bound"
        );
        Ok(Self {
            provider,
            association_json,
            run_id,
            selection_sha256,
            publication_expires_ms,
            maximum_authority_age_ms,
            clock,
            acquired: Mutex::new(None),
        })
    }

    fn sample(&self) -> Result<UtcSample> {
        let sample = self.clock.sample()?;
        ensure!(
            sample.upper_ms < self.publication_expires_ms,
            "original publication expired"
        );
        Ok(sample)
    }

    fn check_association(
        &self,
        selection: &SelectionRecord,
        context: &PreparationContext,
    ) -> Result<()> {
        ensure!(
            context.run_id == self.run_id
                && raw_sha256(&selection.original_bytes) == self.selection_sha256,
            "installed authority native run/selection drift"
        );
        Ok(())
    }
}

fn oldest_lower_ms(sample: &UtcSample) -> Result<u64> {
    Ok(sample.lower_ms)
}

fn validate_age(now_upper_ms: u64, oldest_lower_ms: u64, maximum_age_ms: u64) -> Result<()> {
    let age = now_upper_ms
        .checked_sub(oldest_lower_ms)
        .context("authority acquisition clock ordering unavailable")?;
    ensure!(
        age <= maximum_age_ms,
        "acquired publication authority stale"
    );
    Ok(())
}

impl InstalledSelectedEnvironment for ApplicationSelectedEnvironment {
    fn start_clock_maintenance(&self) -> Result<()> {
        match &self.clock {
            SelectedClock::Calibrated(clock) => clock.start_maintenance(),
            SelectedClock::Kernel(_) => Ok(()),
        }
    }
    fn stop_clock_maintenance(&self) {
        if let SelectedClock::Calibrated(clock) = &self.clock {
            clock.stop_maintenance();
        }
    }

    fn check_current_authority(
        &self,
        selection: &SelectionRecord,
        context: &PreparationContext,
    ) -> Result<()> {
        self.check_association(selection, context)?;
        // Remove the old admission BEFORE any fallible/possibly blocking work.
        *self
            .acquired
            .lock()
            .map_err(|_| anyhow::anyhow!("authority state unavailable"))? = None;
        self.clock.acquire()?;
        let before = self.sample()?;
        ensure!(
            self.provider
                .source_authority_current(self.association_json.clone()),
            "authenticated current source authority unavailable"
        );
        let after = self.sample()?;
        let oldest = oldest_lower_ms(&before)?;
        validate_age(after.upper_ms, oldest, self.maximum_authority_age_ms)?;
        *self
            .acquired
            .lock()
            .map_err(|_| anyhow::anyhow!("authority state unavailable"))? =
            Some(AcquiredAuthority {
                context_sha256: context.raw()?.sha256,
                oldest_utc_lower_ms: oldest,
            });
        Ok(())
    }

    fn current_utc_upper_ms(&self) -> Result<u64> {
        Ok(self.sample()?.upper_ms)
    }

    fn local_write_utc_upper_ms(
        &self,
        selection: &SelectionRecord,
        context: &PreparationContext,
    ) -> Result<u64> {
        self.check_association(selection, context)?;
        // Never wait on an acquisition lock or invoke an app/network callback
        // at the final write boundary. No age reserve/cutoff is renewed here.
        let acquired = self
            .acquired
            .try_lock()
            .map_err(|_| anyhow::anyhow!("authority acquisition pending"))?;
        let acquired = acquired
            .as_ref()
            .context("current source authority not acquired")?;
        let sample = self.sample()?;
        acquired.validate(
            &context.raw()?.sha256,
            sample.upper_ms,
            self.maximum_authority_age_ms,
        )?;
        Ok(sample.upper_ms)
    }
}

#[cfg(test)]
mod controls {
    use super::*;
    #[test]
    fn ordinary_mcp_credentials_cannot_supply_publication_authority() {
        struct OrdinaryMcpProvider;
        impl TokenProvider for OrdinaryMcpProvider {
            fn bearer_token(&self, _: String, _: String) -> Option<String> {
                Some("synthetic-mcp-only".into())
            }
        }
        assert!(!OrdinaryMcpProvider.source_authority_current("{}".into()));
    }

    #[test]
    fn actual_source_constructor_accepts_matched_v3_but_not_v2_or_foreign_manifest() {
        struct Provider;
        impl TokenProvider for Provider {
            fn bearer_token(&self, _: String, _: String) -> Option<String> {
                None
            }
        }
        let selection =
            crate::collection::tests::decode(&crate::collection::tests::body()).unwrap();
        let mut association = serde_json::json!({
            "nativeRunId":"11111111-2222-4333-8444-555555555555",
            "installation":{"installationSha256":"d".repeat(64)},
            "descriptor":{"expiresAt":"2099-01-01T00:00:00.000Z",
                "identity":{"manifestSha256":"d".repeat(64)},
                "source":{"schemaVersion":crate::collection_members::SOURCE_SCHEMA,"origin":"DECLARED",
                    "owner":selection.selection.owner,"fixtureId":"declared","versionId":"a".repeat(64),
                    "selectionRef":selection.selection_ref,"selectionRawSha256":raw_sha256(&selection.original_bytes),
                    "profileId":crate::collection_members::MATCHED_PROFILE,"profileSha256":"c".repeat(64),
                    "installationSha256":"d".repeat(64),"qualification":"unqualified"}}});
        let construct = |value: &Value| {
            ApplicationSelectedEnvironment::new(
                Arc::new(Box::new(Provider)),
                value,
                &selection,
                1000,
                1,
            )
        };
        // Constructor admission only: no sample, network or actual kernel sync.
        assert!(construct(&association).is_ok());
        association["descriptor"]["source"]["profileId"] =
            serde_json::json!(crate::collection_members::N1_OFFER_PROFILE);
        assert!(construct(&association).is_err());
        association["descriptor"]["source"]["profileId"] =
            serde_json::json!(crate::collection_members::PROFILE);
        assert!(construct(&association).is_ok());
        association["descriptor"]["identity"]["manifestSha256"] = serde_json::json!("e".repeat(64));
        assert!(construct(&association).is_err());
    }

    #[test]
    fn oldest_authority_boundary_includes_full_read_wait_without_renewal() {
        assert!(validate_age(1000, 900, 100).is_ok());
        assert!(validate_age(1001, 900, 100).is_err());
        assert!(validate_age(899, 900, 100).is_err());
        // A later retry/final-write time keeps the ORIGINAL acquisition bound.
        assert!(validate_age(1200, 900, 100).is_err());
    }

    #[test]
    fn acquired_authority_cannot_transfer_context_or_extend_age() {
        let acquired = AcquiredAuthority {
            context_sha256: "original-native-context".into(),
            oldest_utc_lower_ms: 900,
        };
        assert!(acquired
            .validate("original-native-context", 1000, 100)
            .is_ok());
        assert!(acquired
            .validate("different-native-context", 1000, 100)
            .is_err());
        assert!(acquired
            .validate("original-native-context", 1001, 100)
            .is_err());
        assert_eq!(acquired.oldest_utc_lower_ms, 900);
    }
}

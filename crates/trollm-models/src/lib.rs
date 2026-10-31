//! trollm-models: the ModelSlot port + MockModel for tests.
//! LlamaCppSlot lives behind the `llama` feature (see docs/ARCHITECTURE-ANDROID.md §9).

#[cfg(feature = "chat-template")]
pub mod chat;
pub mod fence;
#[cfg(feature = "llama")]
pub mod llama;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;
#[cfg(any(feature = "llama", test))]
use std::time::Instant;

#[cfg(any(feature = "llama", test))]
mod deadline;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    Length,
    GrammarReject,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub prompt: String,
    pub grammar_gbnf: Option<String>,
    pub max_tokens: u32,
    /// Cooperative per-generation limit; native decode operations cannot be preempted.
    pub max_duration: Option<Duration>,
    pub stop: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationCapacity {
    pub context_tokens: u32,
    pub context_remaining: u32,
    pub effective_max_tokens: u32,
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub text: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub finish: FinishReason,
    /// Native tokenized context/output capacity; absent for models without this measurement.
    pub capacity: Option<GenerationCapacity>,
    /// Numeric measurements only; unavailable for models without instrumentation.
    pub diagnostics: Option<GenerationDiagnostics>,
}

/// Wall-clock phases, measured with a monotonic clock. Async preparation can
/// overlap other run work and need not fall inside this call's total. Null means the
/// phase was not entered/measured; zero is a measured sub-millisecond phase.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationDiagnostics {
    /// Outer executor measurement, including wrappers and teardown when known.
    pub total_ms: Option<u64>,
    pub weight_verify_ms: Option<u64>,
    pub load_ms: Option<u64>,
    /// Verification/loading ran on a run-owned background worker. Reported on
    /// first consumption; those phase durations may overlap other run work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_async: Option<bool>,
    /// Time this generation call spent awaiting background preparation. Also
    /// reported if its allowance expires before preparation completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_wait_ms: Option<u64>,
    pub prepare_ms: Option<u64>,
    pub prefill_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub settings: GenerationSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSettings {
    pub engine: Option<String>,
    /// Actual device/backend selection is not inferred from build defaults.
    pub backend: Option<String>,
    pub configured_context_tokens: Option<u32>,
    pub configured_threads: Option<i32>,
    pub configured_batch_threads: Option<i32>,
    /// Read from an established native context, not inferred from defaults.
    pub batch_tokens: Option<u32>,
    pub micro_batch_tokens: Option<u32>,
}

impl GenerationDiagnostics {
    /// Preparation phases are separate from inner generation phases.
    pub fn include_loading(&mut self, loading: &Self) {
        self.weight_verify_ms = loading.weight_verify_ms;
        self.load_ms = loading.load_ms;
        self.preparation_async = loading.preparation_async;
        self.preparation_wait_ms = loading.preparation_wait_ms;
    }
}

/// Preserve numeric partial measurements without converting a genuine error
/// into a successful/zero-token model response. Display/source stay unchanged.
#[derive(Debug)]
pub struct ModelDiagnosticError {
    source: anyhow::Error,
    pub diagnostics: GenerationDiagnostics,
}

impl ModelDiagnosticError {
    pub fn attach(source: anyhow::Error, diagnostics: GenerationDiagnostics) -> anyhow::Error {
        Self {
            source,
            diagnostics,
        }
        .into()
    }
}

impl std::fmt::Display for ModelDiagnosticError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}

impl std::error::Error for ModelDiagnosticError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

#[cfg(any(feature = "llama", test))]
#[derive(Clone, Copy)]
pub(crate) enum GenerationPhase {
    Prepare,
    Prefill,
    Generation,
}

/// Small clock helper shared by native instrumentation and deterministic tests.
#[cfg(any(feature = "llama", test))]
pub(crate) struct GenerationTimer {
    pub diagnostics: GenerationDiagnostics,
    active: Option<(GenerationPhase, Instant)>,
}

#[cfg(any(feature = "llama", test))]
impl GenerationTimer {
    pub(crate) fn new() -> Self {
        Self {
            diagnostics: GenerationDiagnostics::default(),
            active: Some((GenerationPhase::Prepare, Instant::now())),
        }
    }

    pub(crate) fn advance(&mut self, phase: GenerationPhase) {
        self.advance_at(Some(phase), Instant::now());
    }

    pub(crate) fn stop(&mut self) {
        self.advance_at(None, Instant::now());
    }

    /// Stop before `?` unwinds native locals; teardown is not phase work.
    pub(crate) fn checked<T, E>(
        &mut self,
        result: std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        if result.is_err() {
            self.stop();
        }
        result
    }

    fn advance_at(&mut self, next: Option<GenerationPhase>, now: Instant) {
        if let Some((phase, started)) = self.active.take() {
            let elapsed = now.saturating_duration_since(started).as_millis() as u64;
            let field = match phase {
                GenerationPhase::Prepare => &mut self.diagnostics.prepare_ms,
                GenerationPhase::Prefill => &mut self.diagnostics.prefill_ms,
                GenerationPhase::Generation => &mut self.diagnostics.generation_ms,
            };
            *field = Some(elapsed);
        }
        self.active = next.map(|phase| (phase, now));
    }
}

pub trait ModelSlot: Send {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse>;
}

/// Scripted model for tests: returns queued responses in order; errors when exhausted.
/// Records every request for assertions.
#[derive(Debug, Default)]
pub struct MockModel {
    pub script: std::collections::VecDeque<String>,
    pub requests: Vec<GenerateRequest>,
}

impl MockModel {
    pub fn scripted(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            script: lines.into_iter().map(Into::into).collect(),
            requests: Vec::new(),
        }
    }

    /// Convenience alias for [`MockModel::scripted`].
    pub fn from_responses(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::scripted(lines)
    }

    /// All requests recorded so far, in order.
    pub fn recorded_requests(&self) -> &[GenerateRequest] {
        &self.requests
    }

    /// How many scripted responses remain.
    pub fn remaining(&self) -> usize {
        self.script.len()
    }
}

impl ModelSlot for MockModel {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        let text = self
            .script
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("MockModel script exhausted"))?;
        let tokens_in = req.prompt.split_whitespace().count() as u32;
        let tokens_out = text.split_whitespace().count() as u32;
        self.requests.push(req);
        Ok(GenerateResponse {
            text,
            tokens_in,
            tokens_out,
            finish: FinishReason::Stop,
            capacity: None,
            diagnostics: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(prompt: &str) -> GenerateRequest {
        GenerateRequest {
            prompt: prompt.to_string(),
            grammar_gbnf: None,
            max_tokens: 16,
            max_duration: None,
            stop: vec![],
        }
    }

    #[test]
    fn phase_clock_partitions_intervals_and_retains_partial_phase() {
        let started = Instant::now();
        let mut timer = GenerationTimer {
            diagnostics: GenerationDiagnostics::default(),
            active: Some((GenerationPhase::Prepare, started)),
        };
        timer.advance_at(
            Some(GenerationPhase::Prefill),
            started + Duration::from_millis(2),
        );
        timer.advance_at(None, started + Duration::from_millis(5));
        assert_eq!(timer.diagnostics.prepare_ms, Some(2));
        assert_eq!(timer.diagnostics.prefill_ms, Some(3));
        assert_eq!(timer.diagnostics.generation_ms, None);
        timer.advance_at(None, started + Duration::from_millis(99));
        assert_eq!(timer.diagnostics.prefill_ms, Some(3));
        let mut partition = GenerationTimer {
            diagnostics: GenerationDiagnostics::default(),
            active: Some((GenerationPhase::Prepare, started)),
        };
        partition.advance_at(
            Some(GenerationPhase::Prefill),
            started + Duration::from_millis(2),
        );
        partition.advance_at(
            Some(GenerationPhase::Generation),
            started + Duration::from_millis(5),
        );
        partition.advance_at(None, started + Duration::from_millis(9));
        assert_eq!(partition.diagnostics.prepare_ms, Some(2));
        assert_eq!(partition.diagnostics.prefill_ms, Some(3));
        assert_eq!(partition.diagnostics.generation_ms, Some(4));
        assert_eq!(
            partition.diagnostics.prepare_ms.unwrap()
                + partition.diagnostics.prefill_ms.unwrap()
                + partition.diagnostics.generation_ms.unwrap(),
            9
        );
        let mut complete = GenerationTimer::new();
        complete.advance(GenerationPhase::Prefill);
        complete.advance(GenerationPhase::Generation);
        complete.stop();
        assert!(complete.diagnostics.prepare_ms.is_some());
        assert!(complete.diagnostics.prefill_ms.is_some());
        assert!(complete.diagnostics.generation_ms.is_some());
    }

    #[test]
    fn typed_failure_retains_partial_measurements_and_original_error() {
        let error = ModelDiagnosticError::attach(
            anyhow::anyhow!("native prefill failed"),
            GenerationDiagnostics {
                prepare_ms: Some(2),
                prefill_ms: Some(3),
                ..Default::default()
            },
        );
        assert_eq!(error.to_string(), "native prefill failed");
        let diagnostic = &error
            .downcast_ref::<ModelDiagnosticError>()
            .unwrap()
            .diagnostics;
        assert_eq!(diagnostic.prefill_ms, Some(3));
        let value = serde_json::to_value(diagnostic).unwrap();
        assert!(value["generationMs"].is_null());
        assert!(value["settings"]["backend"].is_null());
        assert!(error
            .chain()
            .any(|source| source.to_string() == "native prefill failed"));
    }

    #[test]
    fn error_boundary_stops_active_clock_before_caller_teardown() {
        let mut timer = GenerationTimer::new();
        timer.advance(GenerationPhase::Prefill);
        assert_eq!(timer.checked(Ok::<_, &str>(7)), Ok(7));
        assert!(timer.active.is_some());
        assert_eq!(
            timer.checked(Err::<(), _>("decode failed")),
            Err("decode failed")
        );
        assert!(
            timer.active.is_none(),
            "clock must stop before native locals unwind"
        );
        let captured = timer.diagnostics.prefill_ms;
        assert!(captured.is_some());
        timer.advance_at(None, Instant::now() + Duration::from_secs(1));
        assert_eq!(timer.diagnostics.prefill_ms, captured);
        assert!(timer.diagnostics.generation_ms.is_none());
    }

    #[test]
    fn mock_serves_script_in_fifo_order() {
        let mut m = MockModel::scripted(["first", "second"]);
        assert_eq!(m.generate(req("a")).unwrap().text, "first");
        assert_eq!(m.generate(req("b")).unwrap().text, "second");
    }

    #[test]
    fn mock_errors_when_exhausted() {
        let mut m = MockModel::scripted(["only"]);
        assert!(m.generate(req("a")).is_ok());
        let err = m.generate(req("b")).unwrap_err();
        assert!(err.to_string().contains("exhausted"), "got: {err}");
    }

    #[test]
    fn mock_records_requests() {
        let mut m = MockModel::from_responses(["x", "y"]);
        let mut r = req("hello world");
        r.grammar_gbnf = Some("root ::= ...".into());
        r.stop = vec!["STOP".into()];
        r.max_duration = Some(Duration::from_secs(3));
        m.generate(r).unwrap();
        m.generate(req("again")).unwrap();
        assert_eq!(m.recorded_requests().len(), 2);
        assert_eq!(m.recorded_requests()[0].prompt, "hello world");
        assert_eq!(
            m.recorded_requests()[0].grammar_gbnf.as_deref(),
            Some("root ::= ...")
        );
        assert_eq!(
            m.recorded_requests()[0].max_duration,
            Some(Duration::from_secs(3))
        );
        assert_eq!(m.recorded_requests()[1].prompt, "again");
        assert_eq!(m.remaining(), 0);
    }

    #[test]
    fn mock_counts_tokens_by_whitespace() {
        let mut m = MockModel::scripted(["one two three"]);
        let resp = m.generate(req("four five")).unwrap();
        assert_eq!(resp.tokens_in, 2);
        assert_eq!(resp.tokens_out, 3);
        assert_eq!(resp.finish, FinishReason::Stop);
        assert!(resp.diagnostics.is_none());
    }
}

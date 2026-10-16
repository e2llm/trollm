//! trollm-models: the ModelSlot port + MockModel for tests.
//! LlamaCppSlot lives behind the `llama` feature (see docs/ARCHITECTURE-ANDROID.md §9).

#[cfg(feature = "chat-template")]
pub mod chat;
pub mod fence;
#[cfg(feature = "llama")]
pub mod llama;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    Stop,
    Length,
    GrammarReject,
}

#[derive(Debug, Clone)]
pub struct GenerateRequest {
    pub prompt: String,
    pub grammar_gbnf: Option<String>,
    pub max_tokens: u32,
    pub stop: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GenerateResponse {
    pub text: String,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub finish: FinishReason,
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
            stop: vec![],
        }
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
        m.generate(r).unwrap();
        m.generate(req("again")).unwrap();
        assert_eq!(m.recorded_requests().len(), 2);
        assert_eq!(m.recorded_requests()[0].prompt, "hello world");
        assert_eq!(
            m.recorded_requests()[0].grammar_gbnf.as_deref(),
            Some("root ::= ...")
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
    }
}

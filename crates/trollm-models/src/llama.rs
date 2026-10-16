//! `LlamaCppSlot`: a [`ModelSlot`] backed by llama.cpp via the `llama-cpp-2`
//! crate. Only compiled with `--features llama`.
//!
//! Each `generate` call runs against a fresh context (the ModelSlot port is
//! stateless request/response; the ledger owns all conversation state). A
//! per-call GBNF grammar (`GenerateRequest::grammar_gbnf`) is passed straight
//! through to llama.cpp's grammar sampler, constraining the decode.
//!
//! Finish-reason mapping: EOS / stop-string → [`FinishReason::Stop`], hitting
//! `max_tokens` or the context capacity → [`FinishReason::Length`]. llama.cpp
//! has no grammar-rejection signal (constrained decode makes rejection
//! impossible by construction), so this slot NEVER emits
//! [`FinishReason::GrammarReject`].

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use crate::{FinishReason, GenerateRequest, GenerateResponse, ModelSlot};

/// Sampling defaults for [`LlamaCppSlot`]. Low temperature by default: the
/// coder slot's structured output wants determinism.
#[derive(Debug, Clone, Copy)]
pub struct LlamaCppOptions {
    /// Sampling temperature. Default 0.2.
    pub temperature: f32,
    /// Nucleus sampling cutoff. Default 0.95.
    pub top_p: f32,
    /// RNG seed for sampling. Default 0xFFFF_FFFF (llama.cpp's "random").
    pub seed: u32,
}

impl Default for LlamaCppOptions {
    fn default() -> Self {
        Self {
            temperature: 0.2,
            top_p: 0.95,
            seed: 0xFFFF_FFFF,
        }
    }
}

/// A llama.cpp-backed model slot. Loads the GGUF once at construction; creates
/// a fresh inference context per `generate` call.
pub struct LlamaCppSlot {
    // Fields drop in declaration order: models must outlive their contexts,
    // and be freed before the slot releases its backend reference.
    model: LlamaModel,
    backend: Arc<LlamaBackend>,
    path: PathBuf,
    ctx_size: u32,
    options: LlamaCppOptions,
}

// `Send` is required by `ModelSlot`. `LlamaModel` is `Send` upstream; the
// backend handle is a reference to process-global llama.cpp state, and every
// mutating call here goes through `&mut self` with a per-call context, so
// moving the slot between threads is sound.
unsafe impl Send for LlamaCppSlot {}

fn shared_backend() -> Result<Arc<LlamaBackend>> {
    // Keep one process-wide reference so initialization/free cannot race when
    // the last slot closes while another run starts loading a model.
    static BACKEND: Mutex<Option<Arc<LlamaBackend>>> = Mutex::new(None);
    let mut backend = BACKEND
        .lock()
        .map_err(|_| anyhow::anyhow!("llama backend lock poisoned"))?;
    if let Some(backend) = backend.as_ref() {
        return Ok(Arc::clone(backend));
    }
    let initialized = Arc::new(LlamaBackend::init().context("failed to init llama backend")?);
    *backend = Some(Arc::clone(&initialized));
    Ok(initialized)
}

fn ensure_bos(tokens: &mut Vec<LlamaToken>, bos: LlamaToken, should_add: bool) {
    if should_add && tokens.first() != Some(&bos) {
        tokens.insert(0, bos);
    }
}

impl LlamaCppSlot {
    /// Load `path` (a GGUF file) with a context of `ctx` tokens and default
    /// sampling options.
    pub fn new(path: &Path, ctx: u32) -> Result<Self> {
        Self::with_options(path, ctx, LlamaCppOptions::default())
    }

    /// Like [`LlamaCppSlot::new`], with explicit sampling options.
    pub fn with_options(path: &Path, ctx: u32, options: LlamaCppOptions) -> Result<Self> {
        if ctx == 0 || ctx > i32::MAX as u32 {
            bail!(
                "llama context size must be between 1 and {} tokens",
                i32::MAX
            );
        }
        let backend = shared_backend()?;
        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(&backend, path, &model_params)
            .with_context(|| format!("failed to load GGUF model from {}", path.display()))?;
        Ok(Self {
            model,
            backend,
            path: path.to_path_buf(),
            ctx_size: ctx,
            options,
        })
    }

    /// Path of the loaded GGUF (for diagnostics / event-log lines).
    pub fn model_path(&self) -> &Path {
        &self.path
    }

    /// Context size in tokens this slot was constructed with.
    pub fn ctx_size(&self) -> u32 {
        self.ctx_size
    }
}

impl ModelSlot for LlamaCppSlot {
    fn generate(&mut self, req: GenerateRequest) -> Result<GenerateResponse> {
        let vocab = self.model.vocab();
        // Raw ledger/MCP content is one user message. Reserved vocabulary
        // controls inside it could otherwise close that message after the
        // template's control tokens are recognized by the tokenizer.
        if vocab
            .tokenize(req.prompt.as_bytes(), false, true)
            .iter()
            .any(|token| vocab.is_control(*token))
        {
            bail!("rendered prompt contains a reserved model CONTROL token; message boundaries cannot be preserved");
        }
        let ctx_size =
            NonZeroU32::new(self.ctx_size).context("llama context size must be positive")?;
        let ctx_params = LlamaContextParams::default().with_n_ctx(Some(ctx_size));
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .context("failed to create llama context")?;

        let template = self.model.chat_template(None)
            .context("GGUF model has no usable chat template; provide an instruction model with a supported template")?;
        let template = template
            .as_c_str()
            .to_str()
            .context("GGUF chat template is not UTF-8")?;
        let bos = String::from_utf8(if vocab.bos().0 >= 0 {
            vocab.token_to_piece(vocab.bos(), true, None)
        } else {
            Vec::new()
        })
        .context("GGUF BOS token is not UTF-8")?;
        let eos = String::from_utf8(if vocab.eos().0 >= 0 {
            vocab.token_to_piece(vocab.eos(), true, None)
        } else {
            Vec::new()
        })
        .context("GGUF EOS token is not UTF-8")?;
        let prompt = crate::chat::render_chat(template, &req.prompt, &bos, &eos)?;
        // Templates can already include BOS (e.g. Gemma). Recognize their
        // control tokens, then add BOS only if the vocabulary requires it and
        // the template did not supply it. Never append EOS to the open reply.
        let mut prompt_tokens = vocab.tokenize(prompt.as_bytes(), false, true);
        ensure_bos(&mut prompt_tokens, vocab.bos(), vocab.should_add_bos());
        if prompt_tokens.is_empty() {
            bail!("prompt tokenizes to zero tokens");
        }
        if prompt_tokens.len() as u32 >= self.ctx_size {
            bail!(
                "prompt ({} tokens) does not fit context ({} tokens)",
                prompt_tokens.len(),
                self.ctx_size
            );
        }

        // Sampler chain: optional GBNF constraint first, then temperature
        // shaping, then the draw. Grammar-constrained decode is the whole point
        // of the coder slot, so the grammar sampler leads the chain.
        let mut samplers = Vec::new();
        if let Some(grammar) = &req.grammar_gbnf {
            samplers.push(
                LlamaSampler::grammar(&self.model, grammar, "root")
                    .context("invalid GBNF grammar")?,
            );
        }
        samplers.push(LlamaSampler::top_p(self.options.top_p, 1));
        samplers.push(LlamaSampler::temp(self.options.temperature));
        samplers.push(LlamaSampler::dist(self.options.seed));
        let mut sampler = LlamaSampler::chain(samplers, true);

        let batch_size = NonZeroU32::new(ctx.n_batch())
            .context("llama context has zero batch capacity")?
            .get() as usize;
        let mut batch = LlamaBatch::new(batch_size, 1);
        for (chunk_index, chunk) in prompt_tokens.chunks(batch_size).enumerate() {
            batch.clear();
            let start = chunk_index * batch_size;
            for (offset, token) in chunk.iter().enumerate() {
                let position = start + offset;
                let is_last = position == prompt_tokens.len() - 1;
                batch.add(*token, position as i32, &[0], is_last)?;
            }
            ctx.decode(&mut batch)
                .context("llama prompt prefill failed")?;
        }

        let tokens_in = prompt_tokens.len() as u32;
        let max_out = req.max_tokens.min(self.ctx_size - tokens_in);
        let mut text_bytes: Vec<u8> = Vec::new();
        let mut tokens_out = 0u32;
        let mut n_cur = prompt_tokens.len() as i32;
        let finish;

        loop {
            if tokens_out >= max_out {
                finish = FinishReason::Length;
                break;
            }
            // sample() also accepts the token, advancing grammar/sampler state.
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if vocab.is_eog(token) {
                finish = FinishReason::Stop;
                break;
            }
            text_bytes.extend_from_slice(&vocab.token_to_piece(token, false, None));
            tokens_out += 1;

            // Stop-string check on the accumulated text (stop strings may span
            // token boundaries). Truncate at the earliest stop string.
            let text = String::from_utf8_lossy(&text_bytes);
            if let Some((cut, _)) = req
                .stop
                .iter()
                .filter_map(|s| text.find(s).map(|i| (i, s.len())))
                .min_by_key(|(i, _)| *i)
            {
                text_bytes.truncate(cut);
                finish = FinishReason::Stop;
                break;
            }

            batch.clear();
            batch.add(token, n_cur, &[0], true)?;
            n_cur += 1;
            ctx.decode(&mut batch).context("llama decode failed")?;
        }

        Ok(GenerateResponse {
            text: String::from_utf8_lossy(&text_bytes).into_owned(),
            tokens_in,
            tokens_out,
            finish,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_share_a_backend_across_threads_and_reference_drops() {
        let first = shared_backend().unwrap();
        let other = std::thread::spawn(shared_backend).join().unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &other));
        let retained = Arc::downgrade(&first);
        drop(first);
        drop(other);
        let reopened = shared_backend().unwrap();
        assert!(Arc::ptr_eq(&retained.upgrade().unwrap(), &reopened));
    }

    #[test]
    fn bos_is_added_only_when_required_and_not_already_templated() {
        let bos = LlamaToken(1);
        let word = LlamaToken(2);
        let mut tokens = vec![word];
        ensure_bos(&mut tokens, bos, false);
        assert_eq!(tokens, vec![word]);
        ensure_bos(&mut tokens, bos, true);
        assert_eq!(tokens, vec![bos, word]);
        ensure_bos(&mut tokens, bos, true);
        assert_eq!(tokens, vec![bos, word]);
    }

    #[test]
    fn zero_context_is_rejected_before_model_loading() {
        let error = LlamaCppSlot::new(Path::new("does-not-exist.gguf"), 0)
            .err()
            .unwrap();
        assert!(error.to_string().contains("context size"));
    }

    /// Smoke test: needs real weights. Set TROLLM_TEST_MODEL to a GGUF path
    /// to run it; otherwise it passes trivially (docs §11: "llama slot smoke
    /// test behind feature flag, skipped without weights").
    #[test]
    fn generate_smoke_with_real_weights() {
        let Ok(path) = std::env::var("TROLLM_TEST_MODEL") else {
            eprintln!("skipping: TROLLM_TEST_MODEL not set");
            return;
        };
        let mut slot = LlamaCppSlot::new(Path::new(&path), 2048).unwrap();
        let resp = slot
            .generate(GenerateRequest {
                prompt: "The capital of France is".into(),
                grammar_gbnf: None,
                max_tokens: 16,
                stop: vec![],
            })
            .unwrap();
        assert!(matches!(
            resp.finish,
            FinishReason::Stop | FinishReason::Length
        ));
        assert!(resp.tokens_out > 0);
    }

    /// Grammar smoke test: with weights, a fence-constrained generation must
    /// parse through `extract_tool_calls`.
    #[test]
    fn grammar_constrained_generation_parses() {
        let Ok(path) = std::env::var("TROLLM_TEST_MODEL") else {
            eprintln!("skipping: TROLLM_TEST_MODEL not set");
            return;
        };
        let grammar =
            crate::fence::tool_call_grammar(&[("tracker".to_string(), "get_issue".to_string())]);
        let mut slot = LlamaCppSlot::new(Path::new(&path), 2048).unwrap();
        let resp = slot
            .generate(GenerateRequest {
                prompt: "Fetch issue 7 from the tracker. Reply with a tool call.".into(),
                grammar_gbnf: Some(grammar),
                max_tokens: 128,
                stop: vec![],
            })
            .unwrap();
        let calls = crate::fence::extract_tool_calls(&resp.text);
        assert!(
            !calls.is_empty(),
            "expected at least one fence: {:?}",
            resp.text
        );
        assert!(calls.iter().all(Result::is_ok));
    }
}

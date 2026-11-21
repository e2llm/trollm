use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::{header, HeaderMap, StatusCode};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};

use crate::error::{ApiError, Result};

const MAX_SESSIONS: usize = 64;
const SESSION_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_ATTEMPTS: usize = 8;
const LOGIN_WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Sessions {
    tokens: Mutex<VecDeque<Session>>,
    attempts: Mutex<VecDeque<Instant>>,
}

struct Session {
    hash: digest::Digest,
    created: Instant,
}

impl Sessions {
    pub(crate) fn admit_unlock(&self) -> Result<()> {
        let mut attempts = self.attempts.lock().map_err(|_| ApiError::storage())?;
        attempts.retain(|attempt| attempt.elapsed() < LOGIN_WINDOW);
        if attempts.len() >= MAX_ATTEMPTS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "UNLOCK_THROTTLED",
                "Too many sign-in attempts. Wait a minute and try again.",
            ));
        }
        attempts.push_back(Instant::now());
        Ok(())
    }

    pub(crate) fn issue(&self) -> Result<String> {
        let mut random = [0; 32];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| ApiError::storage())?;
        let token = URL_SAFE_NO_PAD.encode(random);
        let mut tokens = self.tokens.lock().map_err(|_| ApiError::storage())?;
        tokens.retain(|session| session.created.elapsed() < SESSION_TTL);
        if tokens.len() >= MAX_SESSIONS {
            tokens.pop_front();
        }
        tokens.push_back(Session {
            hash: digest::digest(&digest::SHA256, token.as_bytes()),
            created: Instant::now(),
        });
        Ok(token)
    }

    pub(crate) fn require(&self, headers: &HeaderMap) -> Result<()> {
        self.check(headers, false)
    }

    pub(crate) fn revoke(&self, headers: &HeaderMap) -> Result<()> {
        self.check(headers, true)
    }

    fn check(&self, headers: &HeaderMap, revoke: bool) -> Result<()> {
        let mut authorization = headers.get_all(header::AUTHORIZATION).iter();
        let token = authorization
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .filter(|value| value.len() == 43)
            .ok_or_else(ApiError::session_required)?;
        if authorization.next().is_some() {
            return Err(ApiError::session_required());
        }
        let hash = digest::digest(&digest::SHA256, token.as_bytes());
        let mut tokens = self.tokens.lock().map_err(|_| ApiError::storage())?;
        tokens.retain(|session| session.created.elapsed() < SESSION_TTL);
        let index = tokens
            .iter()
            .position(|session| session.hash.as_ref() == hash.as_ref());
        if let Some(index) = index {
            if revoke {
                tokens.remove(index);
            }
            Ok(())
        } else {
            Err(ApiError::session_required())
        }
    }
}

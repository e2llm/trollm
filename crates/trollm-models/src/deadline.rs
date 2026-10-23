use anyhow::{bail, Result};
use std::time::{Duration, Instant};

/// A cooperative deadline, checked around native work rather than an interrupt.
pub(crate) struct GenerationDeadline {
    started: Instant,
    limit: Option<Duration>,
}

impl GenerationDeadline {
    pub(crate) fn new(limit: Option<Duration>) -> Self {
        Self {
            started: Instant::now(),
            limit,
        }
    }

    pub(crate) fn check(&self) -> Result<()> {
        self.check_at(Instant::now())
    }

    fn check_at(&self, now: Instant) -> Result<()> {
        if self
            .limit
            .is_some_and(|limit| now.duration_since(self.started) >= limit)
        {
            bail!("model generation deadline exceeded");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_limit_does_not_expire() {
        let deadline = GenerationDeadline::new(None);
        deadline.check().unwrap();
        deadline
            .check_at(deadline.started + Duration::from_secs(601))
            .unwrap();
    }

    #[test]
    fn expires_at_limit_and_rejects_later_work() {
        let deadline = GenerationDeadline::new(Some(Duration::from_secs(2)));
        deadline
            .check_at(deadline.started + Duration::from_secs(1))
            .unwrap();
        for elapsed in [2, 3] {
            let error = deadline
                .check_at(deadline.started + Duration::from_secs(elapsed))
                .unwrap_err();
            assert!(error.to_string().contains("generation deadline exceeded"));
        }
    }

    #[test]
    fn zero_limit_expires_before_generation() {
        GenerationDeadline::new(Some(Duration::ZERO))
            .check()
            .unwrap_err();
    }
}

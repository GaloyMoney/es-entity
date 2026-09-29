use std::{future::Future, time::Duration};

use crate::{dynamic::transient_of, fail::Laned};

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub min_backoff: Duration,
    pub max_backoff: Duration,
    pub jitter_pct: u8,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            min_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_secs(1),
            jitter_pct: 20,
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff capped at `max_backoff`, with up to `jitter_pct`
    /// of jitter subtracted.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(20);
        let base = self
            .min_backoff
            .checked_mul(1u32.checked_shl(exp).unwrap_or(u32::MAX))
            .unwrap_or(self.max_backoff)
            .min(self.max_backoff);

        if self.jitter_pct == 0 {
            return base;
        }
        let jitter_max_nanos =
            (base.as_nanos() as u64).saturating_mul(self.jitter_pct as u64) / 100;
        if jitter_max_nanos == 0 {
            return base;
        }
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        let jitter_nanos = seed % (jitter_max_nanos + 1);
        base.saturating_sub(Duration::from_nanos(jitter_nanos))
    }
}

pub async fn retry<T, E, F, Fut>(policy: &RetryPolicy, op: F) -> Result<T, E::Settled>
where
    E: Laned,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    retry_with(policy, op, tokio::time::sleep).await
}

pub async fn retry_with<T, E, F, Fut, S, SFut>(
    policy: &RetryPolicy,
    mut op: F,
    sleep: S,
) -> Result<T, E::Settled>
where
    E: Laned,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    S: Fn(Duration) -> SFut,
    SFut: Future<Output = ()>,
{
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match op().await {
            Ok(t) => return Ok(t),
            Err(e) => {
                if e.is_transient() && attempt < policy.max_attempts {
                    let transient = transient_of(&e);
                    #[cfg(feature = "tracing")]
                    if let Some(t) = transient {
                        tracing::debug!(attempt, kind = %t.kind, "retrying transient failure");
                    }
                    let delay = transient
                        .and_then(|t| t.retry_after)
                        .unwrap_or_else(|| policy.backoff(attempt));
                    sleep(delay).await;
                    continue;
                }
                return Err(e.settle(attempt));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy {
            jitter_pct: 0,
            ..Default::default()
        };
        assert_eq!(policy.backoff(1), Duration::from_millis(25));
        assert_eq!(policy.backoff(2), Duration::from_millis(50));
        assert_eq!(policy.backoff(3), Duration::from_millis(100));
        assert_eq!(policy.backoff(20), Duration::from_secs(1));
    }

    #[test]
    fn jitter_never_exceeds_the_configured_percentage() {
        let policy = RetryPolicy::default();
        for attempt in 1..8 {
            let base = RetryPolicy {
                jitter_pct: 0,
                ..policy
            }
            .backoff(attempt);
            let jittered = policy.backoff(attempt);
            assert!(jittered <= base);
            let min = base - base * policy.jitter_pct as u32 / 100;
            assert!(jittered >= min, "attempt {attempt}: {jittered:?} < {min:?}");
        }
    }
}

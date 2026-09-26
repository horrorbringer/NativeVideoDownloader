use std::time::Duration;
use crate::error::AppError;

pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(1500),
        }
    }
}

impl RetryPolicy {
    pub fn is_transient_error(error: &AppError) -> bool {
        match error {
            AppError::Cancelled => false,
            AppError::InvalidUrl(_) => false,
            AppError::InvalidPath(_) => false,
            AppError::Http { status, .. } => {
                // 5xx errors or 429 Too Many Requests are transient; 4xx are permanent
                *status >= 500 || *status == 429
            }
            AppError::Network(err) => {
                err.is_timeout() || err.is_connect() || !err.is_status()
            }
            AppError::Io(err) => {
                matches!(
                    err.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::Interrupted
                )
            }
            _ => false,
        }
    }

    pub fn should_retry(&self, error: &AppError, retry_count: u32) -> bool {
        if retry_count >= self.max_retries {
            return false;
        }
        Self::is_transient_error(error)
    }

    pub fn delay_for_retry(&self, retry_count: u32) -> Duration {
        let factor = 2u64.pow(retry_count.min(5));
        self.base_delay * factor as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy() {
        let policy = RetryPolicy::default();
        assert!(!policy.should_retry(&AppError::Cancelled, 0));
        assert!(!policy.should_retry(&AppError::InvalidUrl("test".into()), 0));
        assert!(!policy.should_retry(&AppError::Http { status: 404, message: "Not found".into() }, 0));
        assert!(policy.should_retry(&AppError::Http { status: 503, message: "Unavailable".into() }, 0));
        assert!(!policy.should_retry(&AppError::Http { status: 503, message: "Unavailable".into() }, 3));
    }
}

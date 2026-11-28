//! Spending limits: the guardrail on Runpod calls that cost money.
//!
//! Checked inside the tools before any request that spends is sent, so
//! they hold at every approval level, `yolo` included, where nothing asks
//! the user. Unknown prices are refused, never guessed.

use super::RunpodError;

/// Limits on what the Runpod tools may spend, from integration config.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpendLimits {
    /// Most a pod may cost per hour, in USD (`max_hourly_usd`).
    pub max_hourly_usd: f64,
    /// Most GPUs one pod may have (`max_gpu_count`).
    pub max_gpu_count: u32,
    /// Most pods running at once, counting the one asked for
    /// (`max_running_pods`).
    pub max_running_pods: u32,
    /// Longest a serverless job may run, in seconds (`max_job_seconds`).
    pub max_job_seconds: u64,
}

impl Default for SpendLimits {
    fn default() -> Self {
        Self {
            max_hourly_usd: 1.0,
            max_gpu_count: 1,
            max_running_pods: 2,
            max_job_seconds: 300,
        }
    }
}

/// Runpod's shortest job execution timeout, in seconds.
const MIN_JOB_SECONDS: u64 = 5;

impl SpendLimits {
    /// Limits from config values looked up by field name. Missing,
    /// unreadable or negative values keep their defaults.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let read = |name: &str| get(name).map(|v| v.trim().to_string());
        Self {
            max_hourly_usd: read("max_hourly_usd")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(defaults.max_hourly_usd),
            max_gpu_count: read("max_gpu_count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.max_gpu_count),
            max_running_pods: read("max_running_pods")
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.max_running_pods),
            max_job_seconds: read("max_job_seconds")
                .and_then(|v| v.parse().ok())
                .unwrap_or(defaults.max_job_seconds),
        }
    }

    /// Why a value set for one of the fields isn't usable, if one isn't:
    /// what the Integrations panel shows instead of quietly keeping the
    /// default.
    ///
    /// # Errors
    /// The first field set to something that isn't a non-negative number
    /// (a whole number for all but `max_hourly_usd`).
    pub fn validate(get: impl Fn(&str) -> Option<String>) -> Result<(), String> {
        let set = |name: &str| {
            get(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        if let Some(v) = set("max_hourly_usd") {
            if !v.parse::<f64>().is_ok_and(|n| n.is_finite() && n >= 0.0) {
                return Err(format!(
                    "max_hourly_usd must be a number of dollars, not {v:?}"
                ));
            }
        }
        let whole = [
            ("max_gpu_count", u64::from(u32::MAX)),
            ("max_running_pods", u64::from(u32::MAX)),
            ("max_job_seconds", u64::MAX),
        ];
        for (name, max) in whole {
            if let Some(v) = set(name) {
                if !v.parse::<u64>().is_ok_and(|n| n <= max) {
                    return Err(format!("{name} must be a whole number, not {v:?}"));
                }
            }
        }
        Ok(())
    }

    /// Refuse a pod with more than `max_gpu_count` GPUs.
    pub(crate) fn check_gpu_count(&self, tool: &str, count: u32) -> Result<(), RunpodError> {
        if count > self.max_gpu_count {
            return Err(RunpodError::limit(
                tool,
                format!(
                    "{count} GPUs asked, max_gpu_count is {}; raise max_gpu_count under \
                     Integrations > Runpod to allow more",
                    self.max_gpu_count
                ),
            ));
        }
        Ok(())
    }

    /// Refuse a pod costing more than `max_hourly_usd` an hour, or whose
    /// price is unknown.
    pub(crate) fn check_hourly(
        &self,
        tool: &str,
        what: &str,
        usd: Option<f64>,
    ) -> Result<f64, RunpodError> {
        let Some(usd) = usd.filter(|v| v.is_finite() && *v >= 0.0) else {
            return Err(RunpodError::limit(
                tool,
                format!("Runpod gave no hourly price for {what}, so its cost can't be checked"),
            ));
        };
        if usd > self.max_hourly_usd {
            return Err(RunpodError::limit(
                tool,
                format!(
                    "{what} costs ${usd:.2}/hour, max_hourly_usd is ${:.2}; raise \
                     max_hourly_usd under Integrations > Runpod to allow it",
                    self.max_hourly_usd
                ),
            ));
        }
        Ok(usd)
    }

    /// Refuse one more running pod when `running` already run.
    pub(crate) fn check_running(&self, tool: &str, running: usize) -> Result<(), RunpodError> {
        let allowed = usize::try_from(self.max_running_pods).unwrap_or(usize::MAX);
        if running >= allowed {
            return Err(RunpodError::limit(
                tool,
                format!(
                    "{running} pods are already running, max_running_pods is {}; stop one or \
                     raise max_running_pods under Integrations > Runpod",
                    self.max_running_pods
                ),
            ));
        }
        Ok(())
    }

    /// The execution timeout for a job asking `asked` seconds: at least
    /// Runpod's minimum, never more than `max_job_seconds`.
    pub(crate) fn job_seconds(&self, asked: Option<u64>) -> u64 {
        asked
            .unwrap_or(self.max_job_seconds)
            .max(MIN_JOB_SECONDS)
            .min(self.max_job_seconds)
    }
}

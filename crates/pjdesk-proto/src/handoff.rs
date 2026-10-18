//! What an installer must do after it has swapped the exe.
//!
//! Stopping the service and killing the running process is required to unlock
//! the file. The computer stays reachable only if something starts the service
//! again, or starts the signed-in user's own process when there is no service.

use std::time::Duration;

/// How many times to ask the Service Control Manager to start.
pub const SERVICE_START_ATTEMPTS: u32 = 8;

/// Pause before looking again. Attempt 1 waits 400 ms, then 800 ms, up to 3.2 s.
pub fn start_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(400 * u64::from(attempt.clamp(1, SERVICE_START_ATTEMPTS)))
}

/// One follow-up a couple of seconds later, in case the service exits immediately.
pub const HEALTH_RECHECK: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceOutcome {
    /// No service is installed. The signed-in user's process is what hosts.
    NotInstalled,
    /// The service was running, or a start attempt brought it up.
    Running { attempts: u32 },
    /// The service is installed and every start attempt left it stopped.
    Failed { attempts: u32 },
}

pub fn service_outcome(installed: bool, running: bool, attempts: u32) -> ServiceOutcome {
    if !installed {
        ServiceOutcome::NotInstalled
    } else if running {
        ServiceOutcome::Running { attempts }
    } else {
        ServiceOutcome::Failed { attempts }
    }
}

/// The computer can register again. A running service does that itself. With
/// no service (or a service that failed to start), the user's process has to
/// be running.
pub fn computer_is_back(outcome: ServiceOutcome, user_processes: u32) -> bool {
    match outcome {
        ServiceOutcome::Running { .. } => true,
        ServiceOutcome::NotInstalled | ServiceOutcome::Failed { .. } => user_processes > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_service_is_not_back_until_something_hosts() {
        let down = service_outcome(true, false, SERVICE_START_ATTEMPTS);
        assert_eq!(down, ServiceOutcome::Failed { attempts: SERVICE_START_ATTEMPTS });
        assert!(!computer_is_back(down, 0));
        assert!(computer_is_back(down, 1), "the user process hosts when the service did not start");
        assert!(computer_is_back(service_outcome(true, true, 1), 0));
        assert!(!computer_is_back(service_outcome(false, false, 0), 0));
        assert!(computer_is_back(service_outcome(false, false, 0), 1));
    }

    #[test]
    fn retries_back_off_and_then_stop() {
        assert_eq!(start_retry_delay(1).as_millis(), 400);
        assert_eq!(start_retry_delay(2).as_millis(), 800);
        assert!(start_retry_delay(100).as_millis() <= 3_200);
    }
}

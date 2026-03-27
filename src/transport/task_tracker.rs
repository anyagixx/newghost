// FILE: src/transport/task_tracker.rs
// VERSION: 0.1.0
// START_MODULE_CONTRACT
//   PURPOSE: Provide adapter-scoped task tracking for cleanup-sensitive tests and shutdown coordination.
//   SCOPE: Spawn tracked tasks, expose alive count, and wait for tracked tasks to finish.
//   DEPENDS: std, tokio, tokio-util, thiserror, tracing
//   LINKS: M-WSS-GATEWAY, M-IROH-ADAPTER, V-M-WSS-GATEWAY, V-M-IROH-ADAPTER
// END_MODULE_CONTRACT
//
// START_MODULE_MAP
//   AdapterTaskTracker - wrapper around tokio_util::task::TaskTracker
//   TrackerTimeout - timeout error for close_and_wait
// END_MODULE_MAP
//
// START_CHANGE_SUMMARY
//   LAST_CHANGE: v0.1.1 - Added the missing change summary block so adapter cleanup helpers satisfy GRACE governed-file markup rules.
// END_CHANGE_SUMMARY

use std::future::Future;
use std::time::Duration;

use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::task::TaskTracker;
use tracing::error;

#[derive(Debug)]
pub struct AdapterTaskTracker {
    tracker: TaskTracker,
    adapter_name: &'static str,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TrackerTimeout {
    #[error("adapter tasks did not finish in time")]
    TimedOut,
}

impl AdapterTaskTracker {
    pub fn new(adapter_name: &'static str) -> Self {
        Self {
            tracker: TaskTracker::new(),
            adapter_name,
        }
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.tracker.spawn(future)
    }

    pub fn alive_count(&self) -> usize {
        self.tracker.len()
    }

    pub async fn close_and_wait(&self, timeout: Duration) -> Result<(), TrackerTimeout> {
        self.tracker.close();
        match tokio::time::timeout(timeout, self.tracker.wait()).await {
            Ok(()) => Ok(()),
            Err(_) => {
                error!(
                    adapter = self.adapter_name,
                    alive = self.tracker.len(),
                    "adapter tasks did not finish in time"
                );
                Err(TrackerTimeout::TimedOut)
            }
        }
    }
}

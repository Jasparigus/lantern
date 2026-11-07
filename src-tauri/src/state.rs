use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use tokio::process::Child;

/// Shared control for the single export job: who is running, whether the user
/// asked to stop, and the ffmpeg child currently executing (so cancel kills it).
pub struct ExportCtl {
    running: AtomicBool,
    cancel: AtomicBool,
    child: Mutex<Option<Child>>,
}

impl Default for ExportCtl {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportCtl {
    pub fn new() -> ExportCtl {
        ExportCtl {
            running: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            child: Mutex::new(None),
        }
    }

    /// Returns Err when an export is already in flight.
    pub fn begin(&self) -> Result<(), String> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("A video is already being created.".to_string());
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn end(&self) {
        self.kill_child();
        *self.child.lock().unwrap() = None;
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.kill_child();
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn child_guard(&self) -> MutexGuard<'_, Option<Child>> {
        self.child.lock().unwrap()
    }

    pub fn attach(&self, child: Child) {
        *self.child_guard() = Some(child);
    }

    pub fn detach(&self) {
        // Dropping a tokio Child kills+reaps the process (kill_on_drop).
        let _ = self.child_guard().take();
    }

    fn kill_child(&self) {
        if let Some(child) = self.child_guard().as_mut() {
            let _ = child.start_kill();
        }
    }

    /// `Some(Ok(status))` once the attached child has exited (and is reaped),
    /// `None` while it is still running.
    pub fn try_wait(&self) -> Option<std::io::Result<std::process::ExitStatus>> {
        let mut guard = self.child_guard();
        let child = guard.as_mut()?;
        match child.try_wait() {
            Ok(Some(status)) => {
                let child = guard.take();
                drop(child);
                Some(Ok(status))
            }
            Ok(None) => None,
            Err(e) => {
                let child = guard.take();
                drop(child);
                Some(Err(e))
            }
        }
    }
}

pub struct AppState {
    pub project: Mutex<Option<crate::project::ProjectCtx>>,
    pub export: std::sync::Arc<ExportCtl>,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            project: Mutex::new(None),
            export: std::sync::Arc::new(ExportCtl::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_export_runs_at_a_time() {
        let ctl = ExportCtl::new();
        ctl.begin().unwrap();
        assert!(ctl.begin().is_err(), "second begin must be rejected");
        ctl.end();
        ctl.begin().unwrap();
        assert!(!ctl.cancelled());
        ctl.end();
    }

    #[test]
    fn cancel_flag_resets_on_next_begin() {
        let ctl = ExportCtl::new();
        ctl.begin().unwrap();
        ctl.request_cancel();
        assert!(ctl.cancelled());
        ctl.end();
        ctl.begin().unwrap();
        assert!(!ctl.cancelled());
        ctl.end();
    }
}

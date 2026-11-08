use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use tokio::process::Child;

/// Shared control for the single export job: who is running, whether the user
/// asked to stop, and the ffmpeg children currently executing (so cancel kills
/// them all). Stages attach their child and pass the returned id back to
/// `detach`/`try_wait`, which makes concurrent stages safe.
pub struct ExportCtl {
    running: AtomicBool,
    cancel: AtomicBool,
    next_child_id: AtomicU64,
    children: Mutex<Vec<(u64, Child)>>,
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
            next_child_id: AtomicU64::new(1),
            children: Mutex::new(Vec::new()),
        }
    }

    /// Returns Err when an export is already in flight.
    pub fn begin(&self) -> Result<(), String> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("A video is already being created.".to_string());
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.children.lock().unwrap().clear();
        Ok(())
    }

    pub fn end(&self) {
        self.kill_children();
        self.children.lock().unwrap().clear();
        self.running.store(false, Ordering::SeqCst);
    }

    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.kill_children();
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    fn children_guard(&self) -> MutexGuard<'_, Vec<(u64, Child)>> {
        self.children.lock().unwrap()
    }

    /// Registers a running ffmpeg child; hand the id to `detach`/`try_wait`.
    pub fn attach(&self, child: Child) -> u64 {
        let id = self.next_child_id.fetch_add(1, Ordering::SeqCst);
        self.children_guard().push((id, child));
        id
    }

    pub fn detach(&self, id: u64) {
        // Dropping a tokio Child kills+reaps the process (kill_on_drop).
        let mut guard = self.children_guard();
        if let Some(pos) = guard.iter().position(|(child_id, _)| *child_id == id) {
            guard.swap_remove(pos);
        }
    }

    fn kill_children(&self) {
        for (_, child) in self.children_guard().iter_mut() {
            let _ = child.start_kill();
        }
    }

    /// `Some(Ok(status))` once the child with this id has exited (and is reaped),
    /// `None` while it is still running (or the id is unknown).
    pub fn try_wait(&self, id: u64) -> Option<std::io::Result<std::process::ExitStatus>> {
        let mut guard = self.children_guard();
        let pos = guard.iter().position(|(child_id, _)| *child_id == id)?;
        let child = &mut guard[pos].1;
        match child.try_wait() {
            Ok(Some(status)) => {
                let entry = guard.swap_remove(pos);
                drop(entry);
                Some(Ok(status))
            }
            Ok(None) => None,
            Err(e) => {
                let entry = guard.swap_remove(pos);
                drop(entry);
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

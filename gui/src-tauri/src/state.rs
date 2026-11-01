//! Which operation is running now.
//!
//! The tool runs one operation at a time (scan or refresh). `begin` gives a guard.
//! The guard frees the slot when it is dropped, also after an error or a panic.

use ssd_refresh::control::Control;
use std::sync::{Arc, Mutex, MutexGuard};

type Slot = Arc<Mutex<Option<Arc<Control>>>>;

#[derive(Default, Clone)]
pub struct AppState {
    slot: Slot,
}

pub struct RunGuard {
    slot: Slot,
    pub ctl: Arc<Control>,
    pub process_lock: Option<std::fs::File>,
}

fn lock(slot: &Slot) -> MutexGuard<'_, Option<Arc<Control>>> {
    // The data is a plain pointer, so it is safe to use after another thread panicked.
    slot.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    /// Start an operation. It fails when another operation is running.
    pub fn begin(&self) -> Result<RunGuard, String> {
        let mut g = lock(&self.slot);
        if g.is_some() {
            return Err("Another operation is running. Stop it first.".into());
        }
        let ctl = Control::new(None);
        *g = Some(ctl.clone());
        Ok(RunGuard {
            slot: self.slot.clone(),
            ctl,
            process_lock: None,
        })
    }

    /// The control of the running operation, if there is one.
    pub fn current(&self) -> Option<Arc<Control>> {
        lock(&self.slot).clone()
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        *lock(&self.slot) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::AppState;

    #[test]
    fn only_one_operation_at_a_time() {
        let state = AppState::default();
        let first = state.begin().expect("first operation starts");
        assert!(state.begin().is_err(), "second operation must fail");
        drop(first);
        assert!(state.current().is_none());
        assert!(state.begin().is_ok(), "the slot is free again");
    }
}

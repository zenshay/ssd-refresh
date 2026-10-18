//! Tests use synthetic events and an in-memory failing writer.
use crate::control::Control;
use crate::csv::CsvWriter;
use crate::progress::{Phase, Progress};
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct FailedWriter;
impl Write for FailedWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("Test write error"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn finish_releases_control_and_preserves_the_logger() {
    let control = Control::new(None);
    let messages = Arc::new(AtomicUsize::new(0));
    let received = messages.clone();
    control.set_logger(Arc::new(move |_| {
        received.fetch_add(1, Ordering::SeqCst);
    }));
    let weak = Arc::downgrade(&control);
    let progress = Progress::new(Phase::Scan, 0, 0, control.clone());
    progress.log("Test message");
    progress.finish();
    control.message("After finish");
    assert_eq!(messages.load(Ordering::SeqCst), 2);
    drop(control);
    assert!(weak.upgrade().is_none());
}

#[test]
fn failed_csv_write_stops_events_and_restores_priority() {
    let control = Control::new(None);
    let weak = Arc::downgrade(&control);
    let events = Arc::new(AtomicUsize::new(0));
    let received = events.clone();
    control.set_progress_reporter(Arc::new(move |_| {
        received.fetch_add(1, Ordering::SeqCst);
    }));
    let fail = || -> io::Result<()> {
        let _priority = crate::sys::ThreadIoPriorityGuard::new(crate::common::Priority::VeryLow);
        let _progress = Progress::new(Phase::Refresh { dry_run: true }, 1, 1, control.clone());
        CsvWriter::from_writer(FailedWriter).row(["test".into()])?;
        Ok(())
    };
    assert!(fail().is_err());
    let count = events.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(events.load(Ordering::SeqCst), count);
    drop(control);
    assert!(weak.upgrade().is_none());
    #[cfg(windows)]
    {
        // A second background-mode start fails if the previous guard did not restore it.
        assert!(crate::sys::priority_tests::can_enter_background());
    }
}

#[test]
fn stopped_operation_releases_control() {
    let control = Control::new(None);
    let weak = Arc::downgrade(&control);
    let progress = Progress::new(Phase::Verify, 1, 1, control.clone());
    control.request_stop();
    drop(progress);
    drop(control);
    assert!(weak.upgrade().is_none());
}

#[test]
fn panic_stops_the_worker_and_releases_control() {
    let control = Control::new(None);
    let weak = Arc::downgrade(&control);
    let events = Arc::new(AtomicUsize::new(0));
    let received = events.clone();
    control.set_progress_reporter(Arc::new(move |_| {
        received.fetch_add(1, Ordering::SeqCst);
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _priority = crate::sys::ThreadIoPriorityGuard::new(crate::common::Priority::VeryLow);
        let _progress = Progress::new(Phase::Verify, 1, 1, control.clone());
        panic!("Synthetic panic");
    }));
    assert!(result.is_err());
    let count = events.load(Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(350));
    assert_eq!(events.load(Ordering::SeqCst), count);
    drop(control);
    assert!(weak.upgrade().is_none());
    #[cfg(windows)]
    assert!(crate::sys::priority_tests::can_enter_background());
}

#[test]
fn observers_can_change_their_own_registration() {
    let control = Control::new(None);
    let weak = Arc::downgrade(&control);
    control.set_logger(Arc::new(move |_| {
        weak.upgrade().unwrap().set_logger(Arc::new(|_| {}));
    }));
    control.message("Replace the logger without a lock deadlock");
    let weak = Arc::downgrade(&control);
    control.set_progress_reporter(Arc::new(move |_| {
        weak.upgrade()
            .unwrap()
            .set_progress_reporter(Arc::new(|_| {}));
    }));
    control.report_progress(Default::default());
}

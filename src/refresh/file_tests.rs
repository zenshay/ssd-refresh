use super::*;
use crate::common::{Extent, Layout, Meta};
use crate::reader::ExtentTiming;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

fn record_case(line: String) {
    use std::io::Write;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Ok(path) = std::env::var("SSD_REFRESH_MOCK_TRACE") {
        let _guard = LOCK.lock().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Cached,
    None,
    Busy,
    WritersUnknown,
    FlushOpen,
    FlushError,
    FlushIdentity,
    DataIdentity,
    DataVolume,
    UnknownIdentity,
    FlushChange,
    ReadChange,
    LateBusy,
    LateUnknown,
    PathMissing,
    PathReplacement,
    MetaError,
    LayoutChange,
    ShortBaseline,
    PartialBaseline,
    MissingHash,
    Stop,
    MoveError,
    PartialMove,
    Mismatch,
    WriterDuringMove,
    MtimeDuringMove,
    SizeDuringMove,
    CtimeDuringMove,
    SequenceDuringMove,
    UnknownWriterCtimeDuringMove,
    UnknownWriterSequenceDuringMove,
    PostShort,
    PostPartial,
    PostMissingHash,
    ReadError,
    PostReadError,
    PostMetaError,
    UnknownAfterMove,
    BaselineMetaError,
    DataOpenError,
    LayoutError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Move,
    Flush,
    Data,
    CurrentPath,
}

struct Handle {
    role: Role,
    live: Rc<Cell<usize>>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

struct Mock {
    fault: Fault,
    reads: usize,
    flushes: usize,
    moves: usize,
    opens: usize,
    writer_checks: usize,
    live: Rc<Cell<usize>>,
    ctl: Arc<Control>,
    enabled: bool,
    fast: bool,
    sparse: bool,
}

impl Mock {
    fn new(fault: Fault, ctl: Arc<Control>) -> Self {
        Self {
            fault,
            reads: 0,
            flushes: 0,
            moves: 0,
            opens: 0,
            writer_checks: 0,
            live: Rc::new(Cell::new(0)),
            ctl,
            enabled: true,
            fast: false,
            sparse: false,
        }
    }
    fn handle(&self, role: Role) -> Handle {
        self.live.set(self.live.get() + 1);
        Handle {
            role,
            live: self.live.clone(),
        }
    }
    fn metadata(&self) -> Meta {
        Meta {
            size: 8193,
            id: (12, 34),
            links: 1,
            mtime: 10,
            ctime: 20,
            seq: 30,
            sparse: self.sparse,
            ..Default::default()
        }
    }
}

impl FileOps for Mock {
    type Handle = Handle;
    fn open_meta(&mut self, _: &Path) -> io::Result<Handle> {
        self.opens += 1;
        if self.opens > 1 && self.fault == Fault::PathMissing {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        Ok(self.handle(if self.opens == 1 {
            Role::Move
        } else {
            Role::CurrentPath
        }))
    }
    fn open_data(&mut self, _: &Path) -> io::Result<Handle> {
        if self.fault == Fault::DataOpenError {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(self.handle(Role::Data))
    }
    fn open_flush(&mut self, _: &Path) -> io::Result<Handle> {
        if self.fault == Fault::FlushOpen {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(self.handle(Role::Flush))
    }
    fn meta(&mut self, file: &Handle) -> io::Result<Meta> {
        if self.fault == Fault::BaselineMetaError {
            return Err(io::Error::other("Synthetic baseline metadata error"));
        }
        if self.fault == Fault::PostMetaError && self.moves > 0 {
            return Err(io::Error::other("Synthetic metadata error after the move"));
        }
        if self.fault == Fault::MetaError && self.reads > 0 {
            return Err(io::Error::other("Synthetic metadata error"));
        }
        let mut meta = self.metadata();
        if (self.fault == Fault::FlushIdentity && file.role == Role::Flush)
            || (self.fault == Fault::DataIdentity && file.role == Role::Data)
            || (self.fault == Fault::PathReplacement && file.role == Role::CurrentPath)
        {
            meta.id.1 += 1;
        }
        if self.fault == Fault::UnknownIdentity {
            meta.id.1 = 0;
        }
        if self.fault == Fault::DataVolume && file.role == Role::Data {
            meta.id.0 += 1;
        }
        if (self.fault == Fault::FlushChange && self.flushes > 0)
            || (self.fault == Fault::ReadChange && self.reads > 0)
            || (self.fault == Fault::MtimeDuringMove && self.moves > 0)
        {
            meta.mtime += 1;
        }
        if self.moves > 0 {
            match self.fault {
                Fault::SizeDuringMove => meta.size += 1,
                Fault::CtimeDuringMove | Fault::UnknownWriterCtimeDuringMove => meta.ctime += 1,
                Fault::SequenceDuringMove | Fault::UnknownWriterSequenceDuringMove => meta.seq += 1,
                _ => {}
            }
        }
        Ok(meta)
    }
    fn layout(&mut self, _: &Handle, _: &Meta) -> io::Result<Layout> {
        if self.fault == Fault::LayoutError {
            return Err(io::Error::other("Synthetic layout error"));
        }
        Ok(Layout {
            known: true,
            shared: self.fault == Fault::LayoutChange && self.reads > 0,
            extents: vec![Extent {
                logical: 0,
                len: 12288,
                physical: Some(16384),
            }],
            ..Default::default()
        })
    }
    fn writers(&mut self, _: &Path) -> Option<bool> {
        self.writer_checks += 1;
        match (self.fault, self.writer_checks) {
            (Fault::Busy, 1) | (Fault::LateBusy, 2) | (Fault::WriterDuringMove, 3) => Some(true),
            (Fault::WritersUnknown, 1) | (Fault::LateUnknown, 2) => None,
            (
                Fault::UnknownAfterMove
                | Fault::UnknownWriterCtimeDuringMove
                | Fault::UnknownWriterSequenceDuringMove,
                3,
            ) => None,
            _ => Some(false),
        }
    }
    fn flush(&mut self, _: &Handle) -> io::Result<()> {
        self.flushes += 1;
        if self.fault == Fault::FlushError {
            return Err(io::Error::other("Synthetic flush error"));
        }
        Ok(())
    }
    fn read(&mut self, _: &Handle, meta: &Meta, _: &Layout) -> io::Result<(ReadResult, bool)> {
        self.reads += 1;
        if self.fault == Fault::ReadError {
            return Err(io::Error::other("Synthetic read error"));
        }
        if self.fault == Fault::PostReadError && self.reads > 1 {
            return Err(io::Error::other("Synthetic read error after the move"));
        }
        if self.fault == Fault::Stop {
            self.ctl.request_stop();
        }
        let post = self.reads > 1;
        let missing_hash =
            self.fault == Fault::MissingHash || (post && self.fault == Fault::PostMissingHash);
        let changed_hash = post
            && matches!(
                self.fault,
                Fault::Mismatch
                    | Fault::WriterDuringMove
                    | Fault::MtimeDuringMove
                    | Fault::SizeDuringMove
                    | Fault::CtimeDuringMove
                    | Fault::SequenceDuringMove
                    | Fault::UnknownWriterCtimeDuringMove
                    | Fault::UnknownWriterSequenceDuringMove
                    | Fault::UnknownAfterMove
            );
        let result = ReadResult {
            cached: self.fault == Fault::Cached,
            source: None,
            file_bytes: if self.fault == Fault::PartialBaseline
                || (post && self.fault == Fault::PostPartial)
            {
                4096
            } else {
                meta.size
            },
            disk_bytes: if self.sparse { 4096 } else { 8193 },
            disk_secs: 0.1,
            short_read: self.fault == Fault::ShortBaseline
                || (post && self.fault == Fault::PostShort),
            sha256: (!missing_hash).then_some([if changed_hash { 8 } else { 7 }; 32]),
            extents: vec![ExtentTiming {
                bytes: 8193,
                secs: 0.1,
                slow: !self.fast,
                ..Default::default()
            }],
        };
        Ok((result, !self.fast))
    }
    fn can_move(&self) -> bool {
        self.enabled
    }
    fn relocate(
        &mut self,
        _: &Path,
        file: &Handle,
        _: &Layout,
        stats: &mut MoveStats,
        _: &dyn Fn(&str),
    ) -> io::Result<bool> {
        assert_eq!(file.role, Role::Move);
        self.moves += 1;
        stats.moves += 1;
        stats.moved_bytes += 8193;
        if self.fault == Fault::MoveError {
            return Err(io::Error::other("Synthetic move error"));
        }
        Ok(self.fault != Fault::PartialMove)
    }
}

fn options() -> RefreshOptions {
    RefreshOptions {
        from: "synthetic.csv".into(),
        out: "synthetic.refresh.csv".into(),
        select_all: false,
        force: false,
        confirm: None,
        allow_system_volume: false,
        resume: false,
        dry_run: false,
        stop_after: 0,
        max_chunk_mb: 16,
        control_fraction: 0.0,
        read: ReadOptions::default(),
    }
}

fn execute(mock: &mut Mock, opts: &RefreshOptions) -> Row {
    let mut row = Row {
        path: "synthetic.dat".into(),
        ..Default::default()
    };
    let ctl = mock.ctl.clone();
    if let Err(error) = process_file(
        mock,
        opts,
        Path::new("synthetic.dat"),
        &mut row,
        &ctl,
        &|_| {},
    ) {
        row.status = "ERROR".into();
        row.detail = error.to_string();
    }
    assert_eq!(
        mock.live.get(),
        0,
        "Every handle must close: {:?}",
        mock.fault
    );
    row
}

#[test]
fn unsafe_baselines_never_call_move_even_with_force() {
    for force in [false, true] {
        for (fault, expected) in [
            (Fault::Busy, "SKIP_BUSY"),
            (Fault::WritersUnknown, "SKIP_WRITERS_UNKNOWN"),
            (Fault::FlushOpen, "SKIP_FLUSH"),
            (Fault::FlushError, "SKIP_FLUSH"),
            (Fault::FlushIdentity, "SKIP_IDENTITY"),
            (Fault::DataIdentity, "SKIP_IDENTITY"),
            (Fault::DataVolume, "SKIP_IDENTITY"),
            (Fault::UnknownIdentity, "SKIP_IDENTITY"),
            (Fault::FlushChange, "SKIP_CHANGED"),
            (Fault::ReadChange, "SKIP_CHANGED"),
            (Fault::LateBusy, "SKIP_BUSY"),
            (Fault::LateUnknown, "SKIP_WRITERS_UNKNOWN"),
            (Fault::PathMissing, "SKIP_CHECK"),
            (Fault::PathReplacement, "SKIP_IDENTITY"),
            (Fault::MetaError, "SKIP_CHECK"),
            (Fault::BaselineMetaError, "SKIP_CHECK"),
            (Fault::DataOpenError, "SKIP_CHECK"),
            (Fault::LayoutError, "SKIP_CHECK"),
            (Fault::LayoutChange, "SKIP_CHANGED"),
            (Fault::ShortBaseline, "SKIP_INCOMPLETE"),
            (Fault::Cached, "SKIP_CACHED"),
            (Fault::PartialBaseline, "SKIP_INCOMPLETE"),
            (Fault::MissingHash, "SKIP_INCOMPLETE"),
            (Fault::Stop, "SKIP_STOPPED"),
            (Fault::ReadError, "ERROR"),
        ] {
            let mut mock = Mock::new(fault, Control::new(None));
            let row = execute(&mut mock, &RefreshOptions { force, ..options() });
            assert_eq!(
                row.status, expected,
                "{fault:?}, force={force}: {}",
                row.detail
            );
            assert_eq!(mock.moves, 0, "{fault:?}, force={force}");
            assert_eq!(row.st.moved_bytes, 0);
            record_case(format!(
                "MOCK: {fault:?} force={force} status={} moves={} live={}",
                row.status,
                mock.moves,
                mock.live.get()
            ));
            if fault == Fault::FlushIdentity {
                assert_eq!(mock.flushes, 0);
            }
        }
    }
}

#[test]
fn stable_normal_and_sparse_files_use_the_checked_move_handle() {
    for sparse in [false, true] {
        let mut mock = Mock::new(Fault::None, Control::new(None));
        mock.sparse = sparse;
        let row = execute(&mut mock, &options());
        assert_eq!(row.status, "OK");
        assert_eq!(mock.moves, 1);
        assert_eq!(mock.reads, 2);
        assert_eq!(row.sha_before, row.sha_after);
        assert!(!row.meta_key.is_empty());
        record_case(format!(
            "MOCK: Stable sparse={sparse} status={} moves={} live={}",
            row.status,
            mock.moves,
            mock.live.get()
        ));
    }
}

#[test]
fn outcomes_after_a_move_keep_failures_and_data_changes_separate() {
    for (fault, expected) in [
        (Fault::Mismatch, "MISMATCH"),
        (Fault::WriterDuringMove, "CHANGED"),
        (Fault::MoveError, "MOVE_FAILED"),
        (Fault::PartialMove, "PARTIAL"),
        (Fault::PostShort, "VERIFY_FAILED"),
        (Fault::PostPartial, "VERIFY_FAILED"),
        (Fault::PostMissingHash, "VERIFY_FAILED"),
        (Fault::PostReadError, "VERIFY_FAILED"),
        (Fault::PostMetaError, "VERIFY_FAILED"),
        (Fault::UnknownAfterMove, "VERIFY_FAILED"),
    ] {
        let mut mock = Mock::new(fault, Control::new(None));
        let row = execute(&mut mock, &options());
        assert_eq!(row.status, expected, "{fault:?}");
        assert_eq!(mock.moves, 1);
        assert_eq!(
            mock.reads,
            if fault == Fault::PostMetaError { 1 } else { 2 }
        );
        if expected == "VERIFY_FAILED" {
            assert!(row.meta_key.is_empty());
        }
        record_case(format!(
            "MOCK: {fault:?} status={} moves={} live={}",
            row.status,
            mock.moves,
            mock.live.get()
        ));
    }
}

#[test]
fn only_data_write_evidence_explains_a_hash_difference() {
    for (fault, expected) in [
        (Fault::WriterDuringMove, "CHANGED"),
        (Fault::MtimeDuringMove, "CHANGED"),
        (Fault::SizeDuringMove, "CHANGED"),
        (Fault::CtimeDuringMove, "MISMATCH"),
        (Fault::SequenceDuringMove, "MISMATCH"),
        (Fault::UnknownWriterCtimeDuringMove, "VERIFY_FAILED"),
        (Fault::UnknownWriterSequenceDuringMove, "VERIFY_FAILED"),
    ] {
        let mut mock = Mock::new(fault, Control::new(None));
        let row = execute(&mut mock, &options());
        assert_eq!(row.status, expected, "{fault:?}: {}", row.detail);
        assert_ne!(row.sha_before, row.sha_after);
        assert_eq!(mock.moves, 1);
        if expected == "VERIFY_FAILED" {
            assert!(row.meta_key.is_empty());
        }
    }
}

#[test]
fn fast_control_and_dry_run_do_not_call_move() {
    for expected in ["SKIP_FAST", "CONTROL", "DRYRUN"] {
        let mut mock = Mock::new(Fault::None, Control::new(None));
        mock.fast = expected == "SKIP_FAST";
        mock.enabled = expected != "DRYRUN";
        let row = execute(
            &mut mock,
            &RefreshOptions {
                control_fraction: if expected == "CONTROL" { 1.0 } else { 0.0 },
                ..options()
            },
        );
        assert_eq!(row.status, expected);
        assert_eq!(mock.moves, 0);
        record_case(format!(
            "MOCK: {expected} status={} moves={} live={}",
            row.status,
            mock.moves,
            mock.live.get()
        ));
    }
}

#[test]
fn mismatch_or_failed_post_check_stops_before_the_second_file() {
    use crate::outcome::RunStatus;
    use crate::progress::{Phase, Progress};
    for (fault, expected) in [
        (Fault::Mismatch, RunStatus::DataMismatch),
        (Fault::CtimeDuringMove, RunStatus::DataMismatch),
        (Fault::SequenceDuringMove, RunStatus::DataMismatch),
        (Fault::PostReadError, RunStatus::Stopped),
    ] {
        let ctl = Control::new(None);
        let mut mock = Mock::new(fault, ctl.clone());
        let todo = vec![("first.dat".into(), 8193), ("second.dat".into(), 8193)];
        let progress = Progress::new(Phase::Refresh { dry_run: false }, 2, 16386, ctl.clone());
        let mut calls = 0;
        let result = crate::refresh::process_files(
            &todo,
            0,
            &ctl,
            &progress,
            |_, _| {
                calls += 1;
                execute(&mut mock, &options())
            },
            |_| Ok(()),
        )
        .unwrap();
        progress.finish();
        assert_eq!(result.status, expected);
        assert_eq!(calls, 1);
        assert_eq!(mock.moves, 1);
        record_case(format!(
            "MOCK: StopNextFile-{fault:?} status={expected:?} moves={} files={calls} live={}",
            mock.moves,
            mock.live.get()
        ));
    }
}

#[test]
fn safety_skips_can_be_retried_with_resume() {
    for status in [
        "SKIP_BUSY",
        "SKIP_WRITERS_UNKNOWN",
        "SKIP_FLUSH",
        "SKIP_IDENTITY",
        "SKIP_CHANGED",
        "SKIP_CHECK",
        "SKIP_INCOMPLETE",
        "SKIP_STOPPED",
        "VERIFY_FAILED",
    ] {
        assert!(!crate::refresh::FINAL.contains(&status), "{status}");
    }
}

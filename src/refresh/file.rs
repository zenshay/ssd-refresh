//! Per-file checks and moves. Mocks use the same operation order as the system adapter.
use super::{in_control_group, RefreshOptions, Row, Run};
use crate::common::{hex, Kind, Layout, Meta};
use crate::control::Control;
use crate::reader::{read_file_confirmed, ReadOptions, ReadResult};
use crate::sys::{self, MoveStats, Relocator};
use std::fs::File;
use std::io;
use std::path::Path;

trait FileOps {
    type Handle;
    fn open_meta(&mut self, path: &Path) -> io::Result<Self::Handle>;
    fn open_data(&mut self, path: &Path) -> io::Result<Self::Handle>;
    fn open_flush(&mut self, path: &Path) -> io::Result<Self::Handle>;
    fn meta(&mut self, file: &Self::Handle) -> io::Result<Meta>;
    fn layout(&mut self, file: &Self::Handle, meta: &Meta) -> io::Result<Layout>;
    fn writers(&mut self, path: &Path) -> Option<bool>;
    fn flush(&mut self, file: &Self::Handle) -> io::Result<()>;
    fn read(
        &mut self,
        file: &Self::Handle,
        meta: &Meta,
        layout: &Layout,
    ) -> io::Result<(ReadResult, bool)>;
    fn can_move(&self) -> bool;
    fn relocate(
        &mut self,
        path: &Path,
        file: &Self::Handle,
        layout: &Layout,
        stats: &mut MoveStats,
        log: &dyn Fn(&str),
    ) -> io::Result<bool>;
}

struct SystemOps<'a> {
    q: &'a mut sys::IoQueue,
    reloc: Option<&'a mut Relocator>,
    read: &'a ReadOptions,
    ctl: &'a Control,
    block: u64,
}

impl FileOps for SystemOps<'_> {
    type Handle = File;
    fn open_meta(&mut self, path: &Path) -> io::Result<File> {
        sys::open_meta(path)
    }
    fn open_data(&mut self, path: &Path) -> io::Result<File> {
        sys::open_direct(path)
    }
    fn open_flush(&mut self, path: &Path) -> io::Result<File> {
        sys::open_flush(path)
    }
    fn meta(&mut self, file: &File) -> io::Result<Meta> {
        sys::meta(file)
    }
    fn layout(&mut self, file: &File, meta: &Meta) -> io::Result<Layout> {
        sys::layout(file, meta, self.block)
    }
    fn writers(&mut self, path: &Path) -> Option<bool> {
        sys::has_other_writers(path)
    }
    fn flush(&mut self, file: &File) -> io::Result<()> {
        sys::flush_file(file)
    }
    fn read(
        &mut self,
        file: &File,
        meta: &Meta,
        layout: &Layout,
    ) -> io::Result<(ReadResult, bool)> {
        sys::set_read_priority(self.q, file, self.read.measure_priority);
        read_file_confirmed(
            self.q, file, meta.size, layout, true, true, self.read, self.ctl,
        )
    }
    fn can_move(&self) -> bool {
        self.reloc.is_some()
    }
    fn relocate(
        &mut self,
        path: &Path,
        file: &File,
        layout: &Layout,
        stats: &mut MoveStats,
        log: &dyn Fn(&str),
    ) -> io::Result<bool> {
        self.reloc
            .as_mut()
            .ok_or_else(|| io::Error::other("No move operation is available."))?
            .relocate(path, file, layout, self.ctl, stats, log)
    }
}

pub(super) fn process(
    run: &mut Run<'_>,
    path: &Path,
    row: &mut Row,
    log: &dyn Fn(&str),
) -> io::Result<()> {
    let mut ops = SystemOps {
        q: &mut run.q,
        reloc: run.reloc.as_mut(),
        read: &run.o.read,
        ctl: &run.ctl,
        block: run.vol_block,
    };
    process_file(&mut ops, run.o, path, row, &run.ctl, log)
}

fn skip(row: &mut Row, status: &str, detail: impl Into<String>) {
    row.status = status.into();
    row.detail = detail.into();
}

fn check_writers(ops: &mut impl FileOps, path: &Path, row: &mut Row) -> bool {
    row.writers = ops.writers(path);
    match row.writers {
        Some(false) => true,
        Some(true) => {
            skip(
                row,
                "SKIP_BUSY",
                "The file may be open for writing. Try again after the program closes it.",
            );
            false
        }
        None => {
            skip(
                row,
                "SKIP_WRITERS_UNKNOWN",
                "The tool cannot check for other writers. No data was moved.",
            );
            false
        }
    }
}

fn checked_meta<O: FileOps>(ops: &mut O, file: &O::Handle, expected: &Meta, row: &mut Row) -> bool {
    let actual = match ops.meta(file) {
        Ok(meta) => meta,
        Err(e) => {
            skip(
                row,
                "SKIP_CHECK",
                format!("Cannot check file metadata: {e}"),
            );
            return false;
        }
    };
    if expected.id.1 == 0 || actual.id != expected.id {
        skip(
            row,
            "SKIP_IDENTITY",
            "The file ID changed or is not available. No data was moved.",
        );
        return false;
    }
    if actual.changed_since(expected)
        || actual.reparse != expected.reparse
        || actual.compressed != expected.compressed
        || actual.sparse != expected.sparse
        || actual.encrypted != expected.encrypted
    {
        row.meta_changed = true;
        skip(
            row,
            "SKIP_CHANGED",
            "The file changed before the move. Try again after the program closes it.",
        );
        return false;
    }
    true
}

fn process_file<O: FileOps>(
    ops: &mut O,
    o: &RefreshOptions,
    path: &Path,
    row: &mut Row,
    ctl: &Control,
    log: &dyn Fn(&str),
) -> io::Result<()> {
    let mf = match ops.open_meta(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            skip(row, "SKIP_MISSING", "The file does not exist now.");
            return Ok(());
        }
        Err(e) => {
            skip(
                row,
                "SKIP_CHECK",
                format!("Cannot open the file for metadata checks: {e}"),
            );
            return Ok(());
        }
    };
    let m1 = match ops.meta(&mf) {
        Ok(meta) => meta,
        Err(e) => {
            skip(
                row,
                "SKIP_CHECK",
                format!("Cannot read baseline metadata: {e}"),
            );
            return Ok(());
        }
    };
    row.size = m1.size;
    let l1 = match ops.layout(&mf, &m1) {
        Ok(layout) => layout,
        Err(e) => {
            skip(
                row,
                "SKIP_CHECK",
                format!("Cannot read the baseline layout: {e}"),
            );
            return Ok(());
        }
    };
    let kind = Kind::classify(&m1, &l1);
    if !kind.movable() {
        skip(row, "SKIP_KIND", kind.name());
        return Ok(());
    }
    row.ext_before = l1.on_disk().count();
    if !check_writers(ops, path, row) {
        return Ok(());
    }

    // Check the flush handle before it can write cached data to a path replacement.
    {
        let flush = match ops.open_flush(path) {
            Ok(file) => file,
            Err(e) => {
                skip(
                    row,
                    "SKIP_FLUSH",
                    format!("Cannot open the file for a cache flush: {e}"),
                );
                return Ok(());
            }
        };
        if !checked_meta(ops, &flush, &m1, row) {
            return Ok(());
        }
        if let Err(e) = ops.flush(&flush) {
            skip(row, "SKIP_FLUSH", format!("The cache flush failed: {e}"));
            return Ok(());
        }
    }
    let data = match ops.open_data(path) {
        Ok(file) => file,
        Err(e) => {
            skip(
                row,
                "SKIP_CHECK",
                format!("Cannot open the file for the baseline read: {e}"),
            );
            return Ok(());
        }
    };
    if !checked_meta(ops, &data, &m1, row) || !checked_meta(ops, &mf, &m1, row) {
        return Ok(());
    }
    let (pre, first_slow) = ops.read(&data, &m1, &l1)?;
    row.before_mbps = pre.mbps();
    row.before_slow = pre.is_slow();
    row.before_first_slow = first_slow;
    row.sha_before = pre.sha256.map(|h| hex(&h)).unwrap_or_default();
    if pre.short_read || pre.sha256.is_none() || pre.file_bytes != m1.size {
        skip(
            row,
            "SKIP_INCOMPLETE",
            "The baseline read is incomplete. No data was moved.",
        );
        return Ok(());
    }
    if !checked_meta(ops, &mf, &m1, row) || !checked_meta(ops, &data, &m1, row) {
        return Ok(());
    }
    row.meta_key = m1.key();
    if !o.force && !pre.is_slow() {
        skip(row, "SKIP_FAST", "The file is not slow now.");
        return Ok(());
    }
    if in_control_group(&row.path, o.control_fraction) {
        skip(row, "CONTROL", "control group: read only, not moved");
        return Ok(());
    }
    if !ops.can_move() {
        skip(row, "DRYRUN", "Read only. No data was moved.");
        return Ok(());
    }

    // Check the current path, the held handles, and writers after the baseline read.
    {
        let current = match ops.open_meta(path) {
            Ok(file) => file,
            Err(e) => {
                skip(
                    row,
                    "SKIP_CHECK",
                    format!("Cannot check the path before the move: {e}"),
                );
                return Ok(());
            }
        };
        if !checked_meta(ops, &current, &m1, row) {
            return Ok(());
        }
    }
    let move_layout = ops.layout(&mf, &m1)?;
    if !Kind::classify(&m1, &move_layout).movable() {
        skip(
            row,
            "SKIP_CHANGED",
            "The file layout no longer permits a move.",
        );
        return Ok(());
    }
    if !check_writers(ops, path, row)
        || !checked_meta(ops, &mf, &m1, row)
        || !checked_meta(ops, &data, &m1, row)
    {
        return Ok(());
    }
    if ctl.stopped() {
        skip(row, "SKIP_STOPPED", "The run stopped before the move.");
        return Ok(());
    }
    // These checks do not hold a write lock. A writer can still start after this point.
    let mut complete = true;
    match ops.relocate(path, &mf, &move_layout, &mut row.st, log) {
        Ok(done) => complete = done,
        Err(e) => {
            row.status = "MOVE_FAILED".into();
            row.detail = e.to_string();
        }
    }
    if let Err(error) = finish_move(ops, path, row, &mf, &data, &m1, complete) {
        row.status = "VERIFY_FAILED".into();
        row.meta_key.clear();
        row.detail +=
            &format!("Cannot check data after the move: {error}. Keep the files and log.");
    }
    Ok(())
}

fn finish_move<O: FileOps>(
    ops: &mut O,
    path: &Path,
    row: &mut Row,
    mf: &O::Handle,
    data: &O::Handle,
    m1: &Meta,
    complete: bool,
) -> io::Result<()> {
    let m2 = ops.meta(mf)?;
    let l2 = ops.layout(mf, &m2)?;
    row.ext_after = l2.on_disk().count();
    let (post, first_slow) = ops.read(data, &m2, &l2)?;
    row.after_mbps = post.mbps();
    row.after_slow = post.is_slow();
    row.after_first_slow = first_slow;
    row.sha_after = post.sha256.map(|h| hex(&h)).unwrap_or_default();
    if post.short_read || post.sha256.is_none() || post.file_bytes != m2.size {
        row.meta_key.clear();
        row.status = "VERIFY_FAILED".into();
        row.detail += "The read after the move is incomplete. Keep the files and log.";
        return Ok(());
    }
    let writers_after = ops.writers(path);
    row.writers = writers_after;
    let m3 = ops.meta(mf)?;
    row.meta_changed = m3.changed_since(m1);
    row.meta_key = if m3.changed_since(&m2) {
        String::new()
    } else {
        m2.key()
    };
    // Change time and sequence changes do not prove a data write.
    // Without data write evidence, a hash difference remains a mismatch.
    let data_write = m3.size != m1.size || m3.mtime != m1.mtime;
    let explained = row.writers == Some(true) || data_write;
    if row.sha_before != row.sha_after && row.writers.is_none() && !data_write {
        row.status = "VERIFY_FAILED".into();
        row.meta_key.clear();
        row.detail +=
            "The hash changed, but the writer check is not available. Keep the files and log.";
        return Ok(());
    }
    if row.sha_before != row.sha_after && !explained {
        row.status = "MISMATCH".into();
        row.detail += "The SHA-256 changed without data write evidence. Keep the files and log.";
        return Ok(());
    }
    if row.status == "MOVE_FAILED" {
        return Ok(());
    }
    if !complete {
        row.status = "PARTIAL".into();
        row.detail += "the run stopped during the move; --resume moves the file again";
        return Ok(());
    }
    if row.sha_before == row.sha_after {
        row.status = "OK".into();
    } else {
        row.status = "CHANGED".into();
        row.detail +=
            "The hash changed with a detected writer, size change, or last-write time change.";
    }
    Ok(())
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;

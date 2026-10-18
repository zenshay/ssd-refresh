//! Windows read queue.
use super::*;

// ---------------- overlapped read queue ----------------

enum SlotState {
    Idle,
    Pending,
    Done(io::Result<usize>, Instant),
}

struct Slot {
    ov: Box<OVERLAPPED>,
    event: HANDLE,
    buf: AlignedBuf,
    handle: HANDLE,
    state: SlotState,
}

/// Keeps several overlapped reads in flight; one thread waits for whichever finishes first.
pub struct IoQueue {
    slots: Vec<Slot>,
}

impl IoQueue {
    pub fn new(qd: usize, chunk: usize) -> IoQueue {
        let slots = (0..qd.clamp(1, 64))
            .map(|_| Slot {
                ov: Box::default(),
                event: unsafe { CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null()) },
                buf: AlignedBuf::new(chunk),
                handle: 0,
                state: SlotState::Idle,
            })
            .collect();
        IoQueue { slots }
    }

    pub fn slots(&self) -> usize {
        self.slots.len()
    }

    pub fn buf(&self, slot: usize) -> &[u8] {
        self.slots[slot].buf.as_slice()
    }

    /// On Windows the read priority is set on the file handle (set_file_priority); nothing to do here.
    pub fn set_priority(&mut self, _p: Priority) {}

    pub fn submit(&mut self, f: &File, slot: usize, offset: u64, len: usize) -> io::Result<()> {
        let s = &mut self.slots[slot];
        assert!(matches!(s.state, SlotState::Idle) && len <= s.buf.len());
        *s.ov = OVERLAPPED {
            offset: offset as u32,
            offset_high: (offset >> 32) as u32,
            event: s.event,
            ..Default::default()
        };
        s.handle = raw(f);
        let ok = unsafe {
            ReadFile(
                s.handle,
                s.buf.as_mut_ptr(),
                len as u32,
                std::ptr::null_mut(),
                &mut *s.ov,
            )
        } != 0;
        if ok {
            s.state = SlotState::Pending; // a synchronous completion also sets the event; wait_any collects it
            return Ok(());
        }
        let e = last_err();
        s.state = match e.raw_os_error() {
            Some(ERROR_IO_PENDING) => SlotState::Pending,
            Some(ERROR_HANDLE_EOF) => SlotState::Done(Ok(0), Instant::now()),
            _ => SlotState::Done(Err(e), Instant::now()),
        };
        Ok(())
    }

    /// Waits for any read to finish; returns (slot, bytes read, completion time).
    pub fn wait_any(&mut self) -> (usize, io::Result<usize>, Instant) {
        for (i, s) in self.slots.iter_mut().enumerate() {
            if matches!(s.state, SlotState::Done(..)) {
                let SlotState::Done(r, t) = std::mem::replace(&mut s.state, SlotState::Idle) else {
                    unreachable!()
                };
                return (i, r, t);
            }
        }
        let pending: Vec<usize> = (0..self.slots.len())
            .filter(|&i| matches!(self.slots[i].state, SlotState::Pending))
            .collect();
        assert!(!pending.is_empty(), "no I/O in flight");
        let events: Vec<HANDLE> = pending.iter().map(|&i| self.slots[i].event).collect();
        let r =
            unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, u32::MAX) };
        let t = Instant::now();
        let idx = r as usize;
        if idx >= events.len() {
            return (pending[0], Err(last_err()), t);
        }
        let i = pending[idx];
        let s = &mut self.slots[i];
        let mut n = 0u32;
        let ok = unsafe { GetOverlappedResult(s.handle, &mut *s.ov, &mut n, 0) } != 0;
        s.state = SlotState::Idle;
        if ok {
            return (i, Ok(n as usize), t);
        }
        let e = last_err();
        if e.raw_os_error() == Some(ERROR_HANDLE_EOF) {
            return (i, Ok(0), t);
        }
        (i, Err(e), t)
    }
}

impl Drop for IoQueue {
    fn drop(&mut self) {
        for s in &mut self.slots {
            if matches!(s.state, SlotState::Pending) {
                let mut n = 0u32;
                unsafe {
                    CancelIoEx(s.handle, &mut *s.ov);
                    GetOverlappedResult(s.handle, &mut *s.ov, &mut n, 1);
                }
            }
            unsafe { CloseHandle(s.event) };
        }
    }
}

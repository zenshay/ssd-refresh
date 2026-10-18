//! Windows priority.
use super::*;

pub fn set_file_priority(f: &File, p: Priority) {
    let hint: i32 = match p {
        Priority::VeryLow => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
    };
    // FileIoPriorityHintInfo = 12; failure does not affect correctness
    unsafe { SetFileInformationByHandle(raw(f), 12, (&hint as *const i32).cast(), 4) };
}

/// I/O priority of this thread, including moves the file system performs for it. VeryLow uses
/// background mode; anything else leaves it.
pub fn set_thread_io_priority(p: Priority) {
    let on = p == Priority::VeryLow;
    unsafe {
        SetThreadPriority(
            GetCurrentThread(),
            if on { 0x0001_0000 } else { 0x0002_0000 },
        )
    };
}

/// Own background mode only when this guard starts it.
pub struct ThreadIoPriorityGuard {
    owned: bool,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ThreadIoPriorityGuard {
    pub fn new(priority: Priority) -> Self {
        let owned = priority == Priority::VeryLow
            && unsafe { SetThreadPriority(GetCurrentThread(), 0x0001_0000) } != 0;
        Self {
            owned,
            _thread: std::marker::PhantomData,
        }
    }
}

impl Drop for ThreadIoPriorityGuard {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                SetThreadPriority(GetCurrentThread(), 0x0002_0000);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod priority_tests {
    use super::*;

    pub(crate) fn can_enter_background() -> bool {
        let entered = unsafe { SetThreadPriority(GetCurrentThread(), 0x0001_0000) } != 0;
        if entered {
            unsafe {
                SetThreadPriority(GetCurrentThread(), 0x0002_0000);
            }
        }
        entered
    }

    #[test]
    fn nested_guard_does_not_end_the_outer_background_mode() {
        let _outer = ThreadIoPriorityGuard::new(Priority::VeryLow);
        {
            let _inner = ThreadIoPriorityGuard::new(Priority::VeryLow);
        }
        assert!(!can_enter_background());
    }
}

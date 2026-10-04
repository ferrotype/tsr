//! Fatal-signal breadcrumbs for the concurrent suite's rare signal 11 (#96).
//!
//! With `TSR_TESTRUNNER_TRACE_PHASES` set, SIGSEGV and SIGBUS first write one
//! line to stderr with raw `write(2)` and no allocation: the signal and its
//! code, the faulting address, the faulting thread's id and name, and on Linux
//! its stack pointer, program counter and the distance from the stack pointer
//! down to the faulting address (a small distance means a stack overflow). The
//! previous disposition is then restored and the faulting instruction runs
//! again, so Rust's guard-page check and the default action, with its core,
//! still decide what happens.

#[cfg(unix)]
mod imp {
    use std::sync::OnceLock;

    const SIGNALS: [libc::c_int; 2] = [libc::SIGSEGV, libc::SIGBUS];
    static PREVIOUS: OnceLock<[libc::sigaction; 2]> = OnceLock::new();

    pub fn install() {
        if std::env::var_os("TSR_TESTRUNNER_TRACE_PHASES").is_none() {
            return;
        }
        let mut previous = [unsafe { std::mem::zeroed::<libc::sigaction>() }; 2];
        for (slot, &signal) in previous.iter_mut().zip(&SIGNALS) {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler
                    as extern "C" fn(libc::c_int, *mut libc::siginfo_t, *mut libc::c_void)
                    as usize;
                action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, slot);
            }
        }
        let _ = PREVIOUS.set(previous);
    }

    struct Line {
        bytes: [u8; 512],
        len: usize,
    }
    impl Line {
        fn text(&mut self, text: &[u8]) {
            for &byte in text {
                if self.len < self.bytes.len() {
                    self.bytes[self.len] = byte;
                    self.len += 1;
                }
            }
        }
        fn decimal(&mut self, mut value: u64) {
            let mut digits = [0u8; 20];
            let mut start = digits.len();
            loop {
                start -= 1;
                digits[start] = b'0' + (value % 10) as u8;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            self.text(&digits[start..]);
        }
        fn signed(&mut self, value: i64) {
            if value < 0 {
                self.text(b"-");
            }
            self.decimal(value.unsigned_abs());
        }
        fn hex(&mut self, value: usize) {
            const HEX: &[u8; 16] = b"0123456789abcdef";
            self.text(b"0x");
            let mut started = false;
            for shift in (0..usize::BITS).step_by(4).rev() {
                let digit = (value >> shift) & 15;
                if digit != 0 || started || shift == 0 {
                    started = true;
                    self.text(&[HEX[digit]]);
                }
            }
        }
    }

    fn thread_id() -> u64 {
        #[cfg(target_os = "linux")]
        unsafe {
            libc::syscall(libc::SYS_gettid) as u64
        }
        #[cfg(not(target_os = "linux"))]
        unsafe {
            let mut id = 0u64;
            libc::pthread_threadid_np(0 as libc::pthread_t, &mut id);
            id
        }
    }

    unsafe fn fault_address(info: *const libc::siginfo_t) -> usize {
        #[cfg(target_os = "linux")]
        unsafe {
            (*info).si_addr() as usize
        }
        #[cfg(not(target_os = "linux"))]
        unsafe {
            (*info).si_addr as usize
        }
    }

    /// The faulting thread's stack pointer and program counter.
    #[allow(unused_variables)]
    unsafe fn registers(context: *mut libc::c_void) -> Option<(usize, usize)> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        unsafe {
            let context = &*(context as *const libc::ucontext_t);
            let registers = &context.uc_mcontext.gregs;
            return Some((
                registers[libc::REG_RSP as usize] as usize,
                registers[libc::REG_RIP as usize] as usize,
            ));
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        unsafe {
            let context = &*(context as *const libc::ucontext_t);
            return Some((
                context.uc_mcontext.sp as usize,
                context.uc_mcontext.pc as usize,
            ));
        }
        #[allow(unreachable_code)]
        None
    }

    extern "C" fn handler(
        signal: libc::c_int,
        info: *mut libc::siginfo_t,
        context: *mut libc::c_void,
    ) {
        let mut line = Line {
            bytes: [0; 512],
            len: 0,
        };
        line.text(b"tsr-fatal pid=");
        line.decimal(u64::from(std::process::id()));
        line.text(b" signal=");
        line.signed(i64::from(signal));
        let address = unsafe {
            line.text(b" code=");
            line.signed(i64::from((*info).si_code));
            fault_address(info)
        };
        line.text(b" addr=");
        line.hex(address);
        line.text(b" tid=");
        line.decimal(thread_id());
        let mut name = [0 as libc::c_char; 64];
        if unsafe { libc::pthread_getname_np(libc::pthread_self(), name.as_mut_ptr(), name.len()) }
            == 0
        {
            line.text(b" thread=");
            let bytes = name.iter().take_while(|&&c| c != 0).map(|&c| c as u8);
            for byte in bytes {
                line.text(&[byte]);
            }
        }
        if let Some((stack, program)) = unsafe { registers(context) } {
            line.text(b" sp=");
            line.hex(stack);
            line.text(b" pc=");
            line.hex(program);
            line.text(b" sp-addr=");
            line.signed(stack as i64 - address as i64);
        }
        line.text(b"\n");
        unsafe {
            libc::write(2, line.bytes.as_ptr().cast(), line.len);
        }
        // Restore the previous disposition and return: the instruction faults
        // again under Rust's guard-page check or the default action and core.
        if let Some(previous) = PREVIOUS.get() {
            let index = usize::from(signal != libc::SIGSEGV);
            unsafe {
                libc::sigaction(signal, &previous[index], std::ptr::null_mut());
            }
        }
    }
}

/// Install the fatal-signal breadcrumbs when phase tracing is enabled.
#[doc(hidden)]
pub fn install_fatal_signal_trace() {
    #[cfg(unix)]
    imp::install();
}

/// Diagnostic self-check: fault on a named thread so a workflow can verify the
/// fatal-signal line, the process's exit signal and core collection.
#[doc(hidden)]
pub fn fault_selftest() {
    let _ = std::thread::Builder::new()
        .name("tsr-selftest".into())
        .spawn(|| unsafe { std::ptr::read_volatile(std::ptr::without_provenance::<u64>(8)) })
        .map(std::thread::JoinHandle::join);
}

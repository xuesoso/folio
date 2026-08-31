//! Controlling-terminal liveness: notice when the terminal goes away.
//!
//! When the terminal folio is attached to disappears — the window is closed, the
//! emulator quits or crashes, an ssh link drops — the kernel closes the pty
//! master and every read of the slave returns EOF (or `EIO`) from then on.
//! `poll(2)` reports this as `POLLHUP`.
//!
//! crossterm's Unix event source does not handle it. Its inner read loop treats
//! a 0-byte read as "nothing arrived yet", and on any error other than
//! `WouldBlock`/`Interrupted` it falls through the `match` without breaking — so
//! it calls `read(2)` in a loop that never ends. `event::poll` then never
//! returns, whatever timeout it was handed, and the viewer is left pinned at
//! 100% CPU with no terminal attached: invisible, and drawing power until it is
//! killed by hand.
//!
//! A hangup normally also signals the foreground process group, and folio, which
//! handles no signals, is usually killed by that before the spin can bite. But
//! only the foreground group of that session is signalled, and a parent can
//! catch or ignore the signal on folio's behalf — so "SIGHUP will get us" is not
//! something to rely on, and folio has been seen spinning as an orphan when it
//! didn't.
//!
//! So folio owns the wait: [`wait`] blocks in `poll(2)` on the terminal itself
//! and reports a hangup *as* a hangup, leaving crossterm to read only a live fd.
//! [`watch_hangup`] covers the sliver of time between that check and the read.
//!
//! Owning the wait costs us the `SIGWINCH` wake-up crossterm's event source
//! provides, so [`install_resize_wake`] re-adds it through a self-pipe and
//! [`wait`] reports resizes itself.

#[cfg(unix)]
use std::os::fd::RawFd;
use std::time::Duration;

/// What a [`wait`] ended with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Input is available: crossterm can safely read it.
    Ready,
    /// The window changed size.
    Resized,
    /// Nothing happened before the timeout.
    Timeout,
    /// The terminal hung up. Never read it again — shut down.
    Hangup,
}

#[cfg(unix)]
mod imp {
    use super::{Duration, RawFd, Wait};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// Revents that mean "this terminal is gone for good".
    const DEAD: libc::c_short = libc::POLLHUP | libc::POLLERR | libc::POLLNVAL;

    /// The fd crossterm reads events from: stdin when it is a tty, else
    /// `/dev/tty`. Opened once and deliberately leaked — it lives as long as the
    /// process.
    fn tty_fd() -> Option<RawFd> {
        static FD: OnceLock<Option<RawFd>> = OnceLock::new();
        *FD.get_or_init(|| {
            if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
                return Some(libc::STDIN_FILENO);
            }
            let fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
            (fd >= 0).then_some(fd)
        })
    }

    /// Read end of the `SIGWINCH` self-pipe, or -1 before `install_resize_wake`.
    static WINCH_R: AtomicI32 = AtomicI32::new(-1);
    /// Write end, written from the signal handler.
    static WINCH_W: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn on_winch(_sig: libc::c_int) {
        let fd = WINCH_W.load(Ordering::Relaxed);
        if fd >= 0 {
            // write(2) on a pipe is async-signal-safe. A full pipe (EAGAIN) means
            // a wake-up is already queued, which is all we wanted anyway.
            let byte = 1u8;
            unsafe { libc::write(fd, std::ptr::addr_of!(byte).cast(), 1) };
        }
    }

    pub fn install_resize_wake() {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: fds is a valid 2-element array; pipe only writes into it.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return;
        }
        for fd in fds {
            // Non-blocking so the handler can never stall on a full pipe, and
            // close-on-exec so children don't inherit it.
            unsafe {
                libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        WINCH_R.store(fds[0], Ordering::Relaxed);
        WINCH_W.store(fds[1], Ordering::Relaxed);
        let handler = on_winch as extern "C" fn(libc::c_int) as libc::sighandler_t;
        unsafe { libc::signal(libc::SIGWINCH, handler) };
    }

    fn drain_winch(fd: RawFd) {
        let mut buf = [0u8; 64];
        // SAFETY: buf is a valid writable buffer; the fd is non-blocking.
        while unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }

    pub fn wait(timeout: Duration) -> Wait {
        let Some(tty) = tty_fd() else {
            return Wait::Ready; // nothing to watch: fall back to crossterm
        };
        let winch = WINCH_R.load(Ordering::Relaxed);
        let mut fds = [
            libc::pollfd { fd: tty, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: winch, events: libc::POLLIN, revents: 0 },
        ];
        let n = if winch >= 0 { 2 } else { 1 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: fds holds n valid, initialised pollfds.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), n, ms) };
        if rc < 0 {
            // EINTR or anything else: report a timeout so the caller loops and
            // re-checks its own state, exactly as a quiet tick would.
            return Wait::Timeout;
        }
        // A hangup outranks pending input: those bytes are moot, and reading past
        // them is precisely what wedges crossterm.
        if fds[0].revents & DEAD != 0 {
            return Wait::Hangup;
        }
        if n == 2 && fds[1].revents & libc::POLLIN != 0 {
            drain_winch(winch);
            return Wait::Resized;
        }
        if fds[0].revents & libc::POLLIN != 0 {
            return Wait::Ready;
        }
        Wait::Timeout
    }

    pub fn hung_up() -> bool {
        let Some(fd) = tty_fd() else {
            return false;
        };
        // POLLHUP/POLLERR/POLLNVAL are reported whether or not they were asked
        // for, so requesting no events gives a pure liveness check that never
        // consumes input crossterm needs.
        let mut pfd = libc::pollfd { fd, events: 0, revents: 0 };
        // SAFETY: pfd is a valid, initialised pollfd.
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        rc > 0 && pfd.revents & DEAD != 0
    }

    pub fn watch_hangup(grace: Duration) {
        let Some(fd) = tty_fd() else {
            return;
        };
        std::thread::spawn(move || {
            loop {
                // -1 blocks until something happens to the fd; having requested
                // no events, that can only be a hangup (or a spurious wake).
                let mut pfd = libc::pollfd { fd, events: 0, revents: 0 };
                // SAFETY: pfd is a valid, initialised pollfd.
                let rc = unsafe { libc::poll(&mut pfd, 1, -1) };
                if rc > 0 && pfd.revents & DEAD != 0 {
                    break;
                }
            }
            // The event loop should notice on its own and shut down (saving
            // resume state). If it is wedged anyway — inside crossterm's read
            // loop, or blocked writing to a terminal nobody will drain — nothing
            // will ever free it, so leave rather than burn a core unseen.
            std::thread::sleep(grace);
            // SAFETY: _exit is async-signal-safe and performs no unwinding.
            unsafe { libc::_exit(0) };
        });
    }
}

#[cfg(not(unix))]
mod imp {
    use super::{Duration, Wait};
    pub fn install_resize_wake() {}
    pub fn wait(_timeout: Duration) -> Wait {
        Wait::Ready
    }
    pub fn hung_up() -> bool {
        false
    }
    pub fn watch_hangup(_grace: Duration) {}
}

/// Make window resizes wake [`wait`], via the classic self-pipe trick.
///
/// This replaces crossterm's own `SIGWINCH` handler, so call it *after* the
/// crossterm event source exists (one `event::poll` builds it) — otherwise
/// crossterm's lazy registration clobbers ours instead. Losing crossterm's
/// handler is fine: [`wait`] reports the resize as [`Wait::Resized`].
pub fn install_resize_wake() {
    imp::install_resize_wake();
}

/// Wait up to `timeout` for terminal input, a resize, or a hangup. Only
/// [`Wait::Ready`] means crossterm may read.
pub fn wait(timeout: Duration) -> Wait {
    imp::wait(timeout)
}

/// Has the controlling terminal hung up? Non-blocking.
pub fn hung_up() -> bool {
    imp::hung_up()
}

/// Start the hangup watchdog: force-exit `grace` after the terminal goes away if
/// the event loop hasn't managed to shut down by itself.
pub fn watch_hangup(grace: Duration) {
    imp::watch_hangup(grace);
}

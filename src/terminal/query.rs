//! Synchronous terminal query with a hard deadline.
//!
//! Several capabilities are discovered by writing a request escape sequence and
//! reading the terminal's reply (DA1, `CSI 14 t`/`CSI 16 t`, the kitty graphics
//! query). A terminal that doesn't understand a query simply won't answer, so
//! every read is bounded by a deadline — we must never hang waiting for a reply
//! that will never come.
//!
//! The read is done with `poll(2)` on the controlling TTY (stdin), which is why
//! this is Unix-only. Both target platforms (Linux, macOS) are Unix.

use std::io::Write;
use std::time::{Duration, Instant};

use crate::Result;

/// Write `request`, then read replies from stdin until `is_complete(&buf)` is
/// true or `deadline` elapses. Returns whatever bytes were collected.
///
/// `is_complete` should recognise the *last* expected reply (typically the DA1
/// terminator), so that earlier replies in the same round-trip are guaranteed to
/// have been read before we return.
#[cfg(unix)]
pub fn query<W: Write>(
    out: &mut W,
    request: &[u8],
    deadline: Duration,
    is_complete: impl Fn(&[u8]) -> bool,
) -> Result<Vec<u8>> {
    out.write_all(request)?;
    out.flush()?;

    let fd = libc::STDIN_FILENO;
    let start = Instant::now();
    let mut buf = Vec::with_capacity(64);
    let mut chunk = [0u8; 256];

    loop {
        let elapsed = start.elapsed();
        if elapsed >= deadline {
            break;
        }
        let remaining_ms = (deadline - elapsed).as_millis().min(i32::MAX as u128) as i32;

        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a valid, initialised pollfd; poll only reads/writes it.
        let rc = unsafe { libc::poll(&mut pfd, 1, remaining_ms) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err.into());
        }
        if rc == 0 {
            break; // deadline reached with no data
        }
        if pfd.revents & libc::POLLIN == 0 {
            break; // POLLERR / POLLHUP — nothing more to read
        }

        // SAFETY: chunk is a valid writable buffer of chunk.len() bytes.
        let n = unsafe {
            libc::read(
                fd,
                chunk.as_mut_ptr() as *mut libc::c_void,
                chunk.len(),
            )
        };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if is_complete(&buf) {
            break;
        }
    }

    Ok(buf)
}

/// Non-Unix fallback: send the request but don't attempt to read a reply.
#[cfg(not(unix))]
pub fn query<W: Write>(
    out: &mut W,
    request: &[u8],
    _deadline: Duration,
    _is_complete: impl Fn(&[u8]) -> bool,
) -> Result<Vec<u8>> {
    out.write_all(request)?;
    out.flush()?;
    Ok(Vec::new())
}

/// Discard any bytes already waiting on stdin without blocking.
///
/// Terminal-query replies can dribble in after our deadline; draining them
/// before the event loop starts prevents leftover response bytes (DA1, kitty
/// `OK`, CSI reports) from being mis-parsed as keystrokes.
#[cfg(unix)]
pub fn drain_stdin() {
    let fd = libc::STDIN_FILENO;
    let mut chunk = [0u8; 256];
    loop {
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a valid pollfd; non-blocking poll (timeout 0).
        let rc = unsafe { libc::poll(&mut pfd, 1, 0) };
        if rc <= 0 || pfd.revents & libc::POLLIN == 0 {
            break;
        }
        // SAFETY: chunk is a valid writable buffer.
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if n <= 0 {
            break;
        }
    }
}

#[cfg(not(unix))]
pub fn drain_stdin() {}

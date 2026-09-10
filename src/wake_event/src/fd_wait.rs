use std::ffi::c_int;
use std::os::fd::BorrowedFd;

use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

/// Block until `fd` is readable. Level-triggered, so a signal that lands
/// before the call returns immediately. Returns on any non-`EINTR`
/// `poll` error rather than spinning.
pub fn wait(fd: c_int) {
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let mut fds = [PollFd::new(fd, PollFlags::POLLIN)];
    loop {
        match poll(&mut fds, PollTimeout::NONE) {
            Err(Errno::EINTR) => continue,
            Err(_) => return,
            Ok(_) => {
                if fds[0].revents().is_some_and(|r| !r.is_empty()) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::AsRawFd;

    use super::*;

    #[test]
    fn wait_returns_once_the_fd_is_readable() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        write.write_all(b"x").expect("write");
        // Level-triggered: the byte is already there, so this must not park.
        wait(read.as_raw_fd());
    }

    #[test]
    fn wait_returns_on_a_closed_write_end_rather_than_spinning() {
        // EOF is a POLLHUP/POLLIN readiness, not an error: a caller waiting on
        // a torn-down peer has to be released, not left in the loop.
        let (read, write) = std::io::pipe().expect("pipe");
        drop(write);
        wait(read.as_raw_fd());
    }

    #[test]
    fn wait_is_released_by_a_write_from_another_thread() {
        let (read, mut write) = std::io::pipe().expect("pipe");
        let writer = std::thread::spawn(move || {
            write.write_all(b"x").expect("write");
        });
        wait(read.as_raw_fd());
        writer.join().expect("writer thread");
    }
}

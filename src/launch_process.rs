//! Native process identity and a pre-exec launch-registration handshake.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::{Interest, LoopHandle, Mode, PostAction};

use crate::niri::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub starttime: u64,
}

fn parse_stat(pid: u32, stat: &str) -> Option<(ProcessIdentity, u32)> {
    // comm may contain spaces and parentheses; the last ')' ends field 2.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let parent = fields.nth(1)?.parse().ok()?;
    let starttime = fields.nth(17)?.parse().ok()?;
    Some((ProcessIdentity { pid, starttime }, parent))
}

fn read_stat(pid: u32) -> Option<(ProcessIdentity, u32)> {
    parse_stat(
        pid,
        &std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?,
    )
}

pub fn ancestry(pid: u32) -> Vec<ProcessIdentity> {
    ancestry_with(pid, read_stat)
}

fn ancestry_with(
    mut pid: u32,
    mut read: impl FnMut(u32) -> Option<(ProcessIdentity, u32)>,
) -> Vec<ProcessIdentity> {
    let mut result: Vec<ProcessIdentity> = Vec::new();
    for _ in 0..32 {
        let Some((identity, parent)) = read(pid) else {
            break;
        };
        // A newly reused parent PID cannot be an ancestor of an older child.
        if result
            .last()
            .is_some_and(|child| identity.starttime > child.starttime)
            || result.contains(&identity)
        {
            break;
        }
        result.push(identity);
        if parent == 0 {
            break;
        }
        pid = parent;
    }
    result
}

pub struct LaunchGate {
    child: UnixStream,
    parent_fd: RawFd,
}

/// Install before starting the spawner. Only the compositor thread changes the registry.
pub fn prepare_gate(event_loop: LoopHandle<'static, State>, token: String) -> Option<LaunchGate> {
    let (parent, child) = UnixStream::pair().ok()?;
    parent.set_nonblocking(true).ok()?;
    // A concurrently launched child may inherit this process's parent endpoint.
    // If the compositor exits before acknowledging, EOF is not guaranteed until
    // every such child closes its inherited descriptor, so always bound the wait.
    child.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let parent_fd = parent.as_raw_fd();
    let mut bytes = [0; 4];
    let mut received = 0;
    let source = Generic::new(parent, Interest::READ, Mode::Level);
    event_loop
        .insert_source(source, move |_, socket, state| {
            let mut socket = &**socket;
            match socket.read(&mut bytes[received..]) {
                Ok(0) => return Ok(PostAction::Remove), // Failed spawn / dropped gate.
                Ok(n) => received += n,
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::Interrupted =>
                {
                    return Ok(PostAction::Continue);
                }
                Err(_) => return Ok(PostAction::Remove),
            }
            if received != bytes.len() {
                return Ok(PostAction::Continue);
            }
            if let Some((identity, _)) = read_stat(u32::from_ne_bytes(bytes)) {
                if let Some(intent) = state.niri.launch_tracker.pending(&token, Instant::now()) {
                    intent.process = Some(identity);
                }
            }
            // Release even if the intent expired or procfs is unavailable.
            let _ = socket.write_all(&[1]);
            Ok(PostAction::Remove)
        })
        .ok()?;
    Some(LaunchGate { child, parent_fd })
}

impl LaunchGate {
    /// Called only in the actual grandchild, after double-fork. No allocation or locks.
    pub unsafe fn wait_in_child(&self) {
        // This inherited duplicate would otherwise prevent EOF on compositor exit.
        libc::close(self.parent_fd);
        notify_and_wait(self.child.as_raw_fd());
    }
}

unsafe fn notify_and_wait(fd: RawFd) {
    let bytes = (libc::getpid() as u32).to_ne_bytes();
    let mut written = 0;
    while written < bytes.len() {
        let n = libc::send(
            fd,
            bytes[written..].as_ptr().cast(),
            bytes.len() - written,
            libc::MSG_NOSIGNAL,
        );
        if n > 0 {
            written += n as usize;
        } else if n == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            return;
        }
    }
    let mut ack = 0u8;
    loop {
        let n = libc::read(fd, (&mut ack as *mut u8).cast(), 1);
        if n != -1 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_gate_waits_for_registration_and_releases_on_compositor_disconnect() {
        for acknowledge in [true, false] {
            let (mut parent, child) = UnixStream::pair().unwrap();
            let (done, finished) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                unsafe {
                    notify_and_wait(child.as_raw_fd());
                }
                done.send(()).unwrap();
            });
            let mut pid = [0; 4];
            parent.read_exact(&mut pid).unwrap();
            assert_eq!(u32::from_ne_bytes(pid), std::process::id());
            assert!(matches!(
                finished.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Empty)
            ));
            if acknowledge {
                parent.write_all(&[1]).unwrap();
            }
            drop(parent);
            finished
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            worker.join().unwrap();
        }
    }

    #[test]
    fn exec_gate_fails_open_when_peer_stays_open_without_acknowledging() {
        let (parent, child) = UnixStream::pair().unwrap();
        child
            .set_read_timeout(Some(std::time::Duration::from_millis(30)))
            .unwrap();
        let started = std::time::Instant::now();
        unsafe {
            notify_and_wait(child.as_raw_fd());
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        drop(parent);
    }

    #[test]
    fn stat_comm_may_contain_spaces_and_parentheses() {
        let stat = format!("42 (a ) tricky (name)) S 7 {} 123 0", ["0"; 17].join(" "));
        assert_eq!(
            parse_stat(42, &stat),
            Some((
                ProcessIdentity {
                    pid: 42,
                    starttime: 123
                },
                7
            ))
        );
        assert!(parse_stat(42, "42 (broken) S 7").is_none());
    }

    #[test]
    fn ancestry_rejects_reused_parent_and_bounds_cycles_and_depth() {
        let chain = ancestry_with(9, |pid| {
            Some((ProcessIdentity { pid, starttime: 10 }, pid - 1))
        });
        assert_eq!(chain.len(), 9);
        let reused = ancestry_with(9, |pid| {
            Some((
                ProcessIdentity {
                    pid,
                    starttime: 20 - pid as u64,
                },
                pid - 1,
            ))
        });
        assert_eq!(reused.len(), 1);
        let cycle = ancestry_with(9, |pid| Some((ProcessIdentity { pid, starttime: 10 }, pid)));
        assert_eq!(cycle.len(), 1);
        let deep = ancestry_with(99, |pid| {
            Some((ProcessIdentity { pid, starttime: 10 }, pid - 1))
        });
        assert_eq!(deep.len(), 32);
        assert!(ancestry_with(9, |_| None).is_empty());
    }
}

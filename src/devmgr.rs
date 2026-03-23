use anyhow::{bail, Context, Result};
use nix::cmsg_space;
use nix::sys::socket::{
    recvmsg, sendmsg, socketpair, AddressFamily, ControlMessage, ControlMessageOwned,
    MsgFlags, SockFlag, SockType,
};
use nix::sys::wait::waitpid;
use nix::unistd::{close, fork, getgid, getuid, setgid, setuid, ForkResult, Pid};
use std::io::IoSlice;
use std::io::IoSliceMut;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::process;

const MSG_OPEN: u8 = 1;
const MSG_END: u8 = 2;

pub struct DevMgr {
    sock: OwnedFd,
    child: Pid,
}

impl DevMgr {
    /// Forks a privileged child for opening input devices, then drops root.
    /// Must be called while still running as root (setuid binary).
    pub fn start(devpath: &str) -> Result<Self> {
        if nix::unistd::geteuid().as_raw() != 0 {
            bail!("wshowkeys needs to be setuid to read input events");
        }

        let (parent_sock, child_sock) =
            socketpair(AddressFamily::Unix, SockType::SeqPacket, None, SockFlag::SOCK_CLOEXEC)
                .context("socketpair")?;

        match unsafe { fork() }.context("fork")? {
            ForkResult::Child => {
                drop(parent_sock);
                child_run(child_sock.as_raw_fd(), devpath);
            }
            ForkResult::Parent { child } => {
                drop(child_sock);
                drop_privileges().context("dropping privileges")?;
                Ok(Self {
                    sock: parent_sock,
                    child,
                })
            }
        }
    }

    /// Opens an input device path via the privileged child.
    pub fn open(&self, path: &str) -> Result<OwnedFd> {
        let mut buf = [0u8; 1024];
        buf[0] = MSG_OPEN;
        let path_bytes = path.as_bytes();
        let copy_len = path_bytes.len().min(buf.len() - 1);
        buf[1..1 + copy_len].copy_from_slice(&path_bytes[..copy_len]);

        let iov = [IoSlice::new(&buf[..1 + copy_len])];
        sendmsg::<()>(self.sock.as_raw_fd(), &iov, &[], MsgFlags::empty(), None)
            .context("sendmsg to devmgr")?;

        let mut resp = [0u8; 4];
        let mut cmsg_buf = cmsg_space!(RawFd);
        let found_fd: Option<RawFd> = {
            let mut iov = [IoSliceMut::new(&mut resp)];
            let msg = recvmsg::<()>(
                self.sock.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buf),
                MsgFlags::MSG_CMSG_CLOEXEC,
            )
            .context("recvmsg from devmgr")?;
            let mut out = None;
            for cmsg in msg.cmsgs()? {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    if let Some(&fd) = fds.first() {
                        out = Some(fd);
                    }
                }
            }
            out
        };

        let errno = i32::from_ne_bytes(resp);
        if errno != 0 {
            bail!(
                "devmgr failed to open '{}': {}",
                path,
                std::io::Error::from_raw_os_error(errno)
            );
        }

        if let Some(fd) = found_fd {
            return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
        }

        bail!("devmgr returned success but no fd for '{path}'");
    }
}

impl Drop for DevMgr {
    fn drop(&mut self) {
        let buf = [MSG_END];
        let iov = [IoSlice::new(&buf)];
        let _ = sendmsg::<()>(self.sock.as_raw_fd(), &iov, &[], MsgFlags::empty(), None);

        let mut resp = [0u8; 1];
        let mut iov = [IoSliceMut::new(&mut resp)];
        let _ = recvmsg::<()>(
            self.sock.as_raw_fd(),
            &mut iov,
            None,
            MsgFlags::empty(),
        );

        let _ = waitpid(self.child, None);
    }
}

/// Runs in the forked child (still root). Does not return.
fn child_run(sock: RawFd, devpath: &str) -> ! {
    loop {
        let mut buf = [0u8; 1024];
        let mut iov = [IoSliceMut::new(&mut buf)];
        let msg = match recvmsg::<()>(sock, &mut iov, None, MsgFlags::empty()) {
            Ok(m) if m.bytes > 0 => m,
            _ => process::exit(0),
        };

        let len = msg.bytes;
        match buf[0] {
            MSG_OPEN => {
                let path = match std::str::from_utf8(&buf[1..len]) {
                    Ok(s) => s,
                    Err(_) => process::exit(1),
                };

                if !path.starts_with(devpath) {
                    process::exit(1);
                }

                let (errno, fd) = match nix::fcntl::open(
                    Path::new(path),
                    nix::fcntl::OFlag::O_RDONLY
                        | nix::fcntl::OFlag::O_CLOEXEC
                        | nix::fcntl::OFlag::O_NOCTTY
                        | nix::fcntl::OFlag::O_NONBLOCK,
                    nix::sys::stat::Mode::empty(),
                ) {
                    Ok(fd) => (0i32, Some(fd)),
                    Err(e) => (e as i32, None),
                };

                let resp = errno.to_ne_bytes();
                let iov = [IoSlice::new(&resp)];
                let cmsgs_storage = fd.map(|f| [f]);
                let cmsgs: Vec<ControlMessage> = cmsgs_storage
                    .as_ref()
                    .map_or(vec![], |arr| vec![ControlMessage::ScmRights(arr)]);
                let _ = sendmsg::<()>(sock, &iov, &cmsgs, MsgFlags::empty(), None);

                if let Some(fd) = fd {
                    let _ = close(fd);
                }
            }
            MSG_END => {
                let iov = [IoSlice::new(&[])];
                let _ = sendmsg::<()>(sock, &iov, &[], MsgFlags::empty(), None);
                process::exit(0);
            }
            _ => process::exit(1),
        }
    }
}

fn drop_privileges() -> Result<()> {
    let real_gid = getgid();
    let real_uid = getuid();

    setgid(real_gid).context("setgid")?;
    setuid(real_uid).context("setuid")?;

    if setuid(nix::unistd::Uid::from_raw(0)).is_ok() {
        bail!("failed to drop root -- setuid(0) still succeeds");
    }

    Ok(())
}

use std::mem::size_of_val;
use std::net::UdpSocket;
use std::os::fd::AsRawFd;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};

const SO_TIMESTAMP: libc::c_int = 29;
const SOF_TIMESTAMPING_OPT_TX_SWHW: libc::c_int = 1 << 14;

static SOCKET: OnceLock<Mutex<Option<UdpSocket>>> = OnceLock::new();

pub fn apply() -> Result<()> {
    let mut held = socket_slot().lock().unwrap();
    if held.is_some() {
        return Ok(());
    }
    let socket = UdpSocket::bind("0.0.0.0:0").context("Failed to create RTENTSK UDP socket")?;
    let value = SOF_TIMESTAMPING_OPT_TX_SWHW;
    // SAFETY: `socket` owns a valid file descriptor for the duration of this call,
    // and the pointer/length describe a live `c_int` value as required by setsockopt.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_TIMESTAMP,
            (&value as *const libc::c_int).cast(),
            size_of_val(&value) as libc::socklen_t,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("Failed to enable the RTENTSK timestamping static key");
    }
    *held = Some(socket);
    Ok(())
}

pub fn cleanup() {
    *socket_slot().lock().unwrap() = None;
}

pub fn verify() -> bool {
    socket_slot().lock().unwrap().is_some()
}

fn socket_slot() -> &'static Mutex<Option<UdpSocket>> {
    SOCKET.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_lifetime_tracks_apply_and_cleanup() {
        cleanup();
        apply().unwrap();
        assert!(verify());
        apply().unwrap();
        assert!(verify());
        cleanup();
        assert!(!verify());
    }
}

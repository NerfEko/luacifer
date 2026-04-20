use std::{
    io::{BufReader, Read, Write},
    os::fd::AsFd,
    os::fd::OwnedFd,
};

use wayland_client::{
    Dispatch, QueueHandle,
    protocol::{wl_buffer, wl_shm, wl_shm_pool},
};

/// Create a (read_fd, write_fd) pipe for offer payload delivery.
///
/// The caller passes `write_fd` to `offer.receive(mime, write_fd.as_fd())` and
/// then drops it immediately so EOF propagates after the compositor writes.
/// The caller reads from `read_fd`.
pub fn create_payload_pipe() -> Result<(OwnedFd, OwnedFd), String> {
    nix::unistd::pipe().map_err(|e| format!("pipe: {e}"))
}

/// Read all bytes from the read end of a pipe and return them.
///
/// The write end must already have been dropped by the caller before calling
/// this so the read terminates on EOF rather than blocking indefinitely.
pub fn read_from_pipe(read_fd: OwnedFd) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut reader = BufReader::new(std::fs::File::from(read_fd));
    reader
        .read_to_end(&mut buf)
        .map_err(|e| format!("read payload: {e}"))?;
    Ok(buf)
}

/// Write `payload` to an OwnedFd (the write end of a send pipe).
pub fn write_payload_to_fd(fd: OwnedFd, payload: &[u8]) -> Result<usize, String> {
    use std::io::Write;
    let mut file = std::fs::File::from(fd);
    file.write_all(payload)
        .map_err(|e| format!("send write: {e}"))?;
    Ok(payload.len())
}

/// Check whether `desired` is in `available`, building a descriptive error if not.
pub fn check_mime_available(desired: &str, available: &[String]) -> Result<(), String> {
    if available.iter().any(|m| m == desired) {
        Ok(())
    } else {
        Err(format!(
            "offer does not contain desired mime '{desired}'; available: {available:?}"
        ))
    }
}

pub fn create_single_pixel_buffer<D>(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<D>,
) -> Result<wl_buffer::WlBuffer, String>
where
    D: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
{
    let mut file = tempfile::tempfile().map_err(|e| format!("temp shm: {e}"))?;
    file.write_all(&[0x20, 0x20, 0x20, 0xFF])
        .map_err(|e| format!("write shm pixel: {e}"))?;
    file.flush().map_err(|e| format!("flush shm pixel: {e}"))?;
    let pool = shm.create_pool(file.as_fd(), 4, qh, ());
    Ok(pool.create_buffer(0, 1, 1, 4, wl_shm::Format::Argb8888, qh, ()))
}

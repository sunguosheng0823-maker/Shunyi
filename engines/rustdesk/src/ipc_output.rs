//! Keep dependency diagnostics off the binary protocol stream, including DXGI fallback logs.
use std::{fs::File, io, sync::OnceLock};

static PROTOCOL: OnceLock<File> = OnceLock::new();

pub fn writer() -> io::Result<tokio::fs::File> {
    if PROTOCOL.get().is_none() {
        let file = redirect()?;
        let _ = PROTOCOL.set(file);
    }
    Ok(tokio::fs::File::from_std(
        PROTOCOL.get().unwrap().try_clone()?,
    ))
}

#[cfg(unix)]
fn redirect() -> io::Result<File> {
    use std::os::fd::FromRawFd;
    // Preserve the parent pipe before redirecting ordinary Rust/C stdout to stderr.
    let fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(windows)]
fn redirect() -> io::Result<File> {
    use std::{ffi::c_void, os::windows::io::FromRawHandle};
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn GetStdHandle(kind: u32) -> Handle;
        fn SetStdHandle(kind: u32, handle: Handle) -> i32;
        fn DuplicateHandle(
            source: Handle,
            handle: Handle,
            target: Handle,
            copy: *mut Handle,
            access: u32,
            inherit: i32,
            options: u32,
        ) -> i32;
    }
    unsafe {
        let process = GetCurrentProcess();
        let mut duplicate = std::ptr::null_mut();
        if DuplicateHandle(
            process,
            GetStdHandle(-11_i32 as u32),
            process,
            &mut duplicate,
            0,
            0,
            2,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let file = File::from_raw_handle(duplicate);
        if SetStdHandle(-11_i32 as u32, GetStdHandle(-12_i32 as u32)) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(file)
    }
}

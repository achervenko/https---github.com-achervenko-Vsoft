use std::{io, path::Path};

#[cfg(windows)]
mod windows {
    use super::*;
    use std::{
        ffi::c_void,
        fs::{File, OpenOptions},
        os::windows::io::AsRawHandle,
    };

    #[link(name = "kernel32")]
    extern "system" {
        fn LockFile(
            handle: *mut c_void,
            offset_low: u32,
            offset_high: u32,
            len_low: u32,
            len_high: u32,
        ) -> i32;
        fn UnlockFile(
            handle: *mut c_void,
            offset_low: u32,
            offset_high: u32,
            len_low: u32,
            len_high: u32,
        ) -> i32;
    }

    pub(crate) struct PackageLock(File);

    impl PackageLock {
        pub(crate) fn acquire(destination: &Path) -> io::Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(destination.join("_work").join("package.lock"))?;
            // SAFETY: file owns this synchronous handle. Offset 0 and length 1
            // match the byte locked by Python's msvcrt.locking.
            if unsafe { LockFile(file.as_raw_handle(), 0, 0, 1, 0) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(file))
        }
    }

    impl Drop for PackageLock {
        fn drop(&mut self) {
            // SAFETY: the File is still open and owns the locked byte.
            unsafe { UnlockFile(self.0.as_raw_handle(), 0, 0, 1, 0) };
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::PackageLock;

#[cfg(not(windows))]
pub(crate) struct PackageLock;

#[cfg(not(windows))]
impl PackageLock {
    pub(crate) fn acquire(_destination: &Path) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the converter uses Windows package locking",
        ))
    }
}

//! Phase 17 — OS I/O backends. `io_uring` is Linux-only (feature `io-uring`).
//! Other platforms use pread/seek fallbacks — never pretend io_uring exists.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Bounded async-looking file ops used by region loaders.
pub trait RandomAccessFile: Send {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<usize>;
    fn len(&self) -> io::Result<u64>;
}

/// Portable backend (Windows / macOS / Linux without io_uring).
pub struct StdRandomAccess {
    file: File,
}

impl StdRandomAccess {
    pub fn open(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: File::options().read(true).write(true).create(true).open(path)?,
        })
    }

    pub fn open_readonly(path: &Path) -> io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
        })
    }
}

impl RandomAccessFile for StdRandomAccess {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read(buf)
    }

    fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<usize> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write(buf)
    }

    fn len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }
}

/// Preferred factory: io_uring on Linux+feature, else std.
pub fn open_random_access(path: &Path) -> io::Result<Box<dyn RandomAccessFile>> {
    #[cfg(all(target_os = "linux", feature = "io-uring"))]
    {
        return Ok(Box::new(uring::UringFile::open(path)?));
    }
    #[cfg(not(all(target_os = "linux", feature = "io-uring")))]
    {
        Ok(Box::new(StdRandomAccess::open(path)?))
    }
}

pub fn io_backend_name() -> &'static str {
    #[cfg(all(target_os = "linux", feature = "io-uring"))]
    {
        "io_uring"
    }
    #[cfg(not(all(target_os = "linux", feature = "io-uring")))]
    {
        "std_seek"
    }
}

#[cfg(all(target_os = "linux", feature = "io-uring"))]
mod uring {
    use super::*;
    use io_uring::{opcode, types, IoUring};
    use std::os::fd::{AsRawFd, RawFd};

    /// Single-op submit/wait wrapper — production path for large region reads.
    pub struct UringFile {
        file: File,
        ring: IoUring,
        fd: RawFd,
    }

    impl UringFile {
        pub fn open(path: &Path) -> io::Result<Self> {
            let file = File::options().read(true).write(true).create(true).open(path)?;
            let fd = file.as_raw_fd();
            let ring = IoUring::new(8).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            Ok(Self { file, ring, fd })
        }

        fn submit_rw(
            &mut self,
            write: bool,
            offset: u64,
            buf: *mut u8,
            len: u32,
        ) -> io::Result<usize> {
            let entry = if write {
                opcode::Write::new(types::Fd(self.fd), buf, len)
                    .offset(offset)
                    .build()
                    .user_data(1)
            } else {
                opcode::Read::new(types::Fd(self.fd), buf, len)
                    .offset(offset)
                    .build()
                    .user_data(1)
            };
            unsafe {
                self.ring
                    .submission()
                    .push(&entry)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            }
            self.ring.submit_and_wait(1)?;
            let cqe = self
                .ring
                .completion()
                .next()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no cqe"))?;
            let res = cqe.result();
            if res < 0 {
                return Err(io::Error::from_raw_os_error(-res));
            }
            Ok(res as usize)
        }
    }

    impl RandomAccessFile for UringFile {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            self.submit_rw(false, offset, buf.as_mut_ptr(), buf.len() as u32)
        }

        fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<usize> {
            self.submit_rw(true, offset, buf.as_ptr() as *mut u8, buf.len() as u32)
        }

        fn len(&self) -> io::Result<u64> {
            Ok(self.file.metadata()?.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn std_roundtrip() {
        let dir = std::env::temp_dir().join("rsift_platform_io");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("t.bin");
        {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"hello-world").unwrap();
        }
        let mut ra = StdRandomAccess::open_readonly(&path).unwrap();
        let mut buf = [0u8; 5];
        assert_eq!(ra.read_at(0, &mut buf).unwrap(), 5);
        assert_eq!(&buf, b"hello");
        assert_eq!(io_backend_name().is_empty(), false);
    }
}

//! High-level filesystem convenience functions built on top of bun_sys primitives.
//!
//! These mirror `std::fs` convenience APIs but route through bun_sys syscalls,
//! ensuring consistent error handling and bypassing the Rust standard library.

use bun_core::Mode;
use bun_core::ZBox;

// Re-export File and Stat for convenience
pub use super::file::File;
pub use super::PosixStat;

/// Convert a path string to a ZBox (NUL-terminated) for bun_sys APIs.
#[inline]
fn path_to_zbox(path: &str) -> ZBox {
    ZBox::from_bytes(path.as_bytes())
}

/// Read the entire contents of a file into a string.
///
/// Equivalent to `std::fs::read_to_string(path)`.
pub fn read_to_string(path: &str) -> Result<String, std::io::Error> {
    let zpath = path_to_zbox(path);
    let file = File::open(zpath.as_zstr(), libc::O_RDONLY | libc::O_CLOEXEC, 0)
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))?;

    let bytes = file
        .read_to_end()
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))?;

    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Read the entire contents of a file into a byte vector.
///
/// Equivalent to `std::fs::read(path)`.
pub fn read(path: &str) -> Result<Vec<u8>, std::io::Error> {
    let zpath = path_to_zbox(path);
    let file = File::open(zpath.as_zstr(), libc::O_RDONLY | libc::O_CLOEXEC, 0)
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))?;

    file.read_to_end()
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))
}

/// Write a slice as the entire contents of a file.
///
/// Equivalent to `std::fs::write(path, data)`.
pub fn write(path: &str, data: &[u8]) -> Result<(), std::io::Error> {
    let zpath = path_to_zbox(path);
    let file = File::open(
        zpath.as_zstr(),
        libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC,
        0o666 as Mode,
    )
    .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))?;

    file.write_all(data)
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))
}

/// Query metadata about a file.
///
/// Equivalent to `std::fs::metadata(path)`.
pub fn metadata(path: &str) -> Result<PosixStat, std::io::Error> {
    let zpath = path_to_zbox(path);
    super::stat(zpath.as_zstr())
        .map(|s| PosixStat::init(&s))
        .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))
}

/// Options and flags which can be used to configure how a file is opened.
///
/// Equivalent to `std::fs::OpenOptions`.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
    create_new: bool,
    mode: Mode,
}

impl OpenOptions {
    pub fn new() -> Self {
        OpenOptions {
            read: false,
            write: false,
            append: false,
            truncate: false,
            create: false,
            create_new: false,
            mode: 0o666 as Mode,
        }
    }

    pub fn read(&mut self, read: bool) -> &mut Self {
        self.read = read;
        self
    }

    pub fn write(&mut self, write: bool) -> &mut Self {
        self.write = write;
        self
    }

    pub fn append(&mut self, append: bool) -> &mut Self {
        self.append = append;
        self
    }

    pub fn truncate(&mut self, truncate: bool) -> &mut Self {
        self.truncate = truncate;
        self
    }

    pub fn create(&mut self, create: bool) -> &mut Self {
        self.create = create;
        self
    }

    pub fn create_new(&mut self, create_new: bool) -> &mut Self {
        self.create_new = create_new;
        self
    }

    pub fn mode(&mut self, mode: Mode) -> &mut Self {
        self.mode = mode;
        self
    }

    pub fn open(&self, path: &str) -> Result<File, std::io::Error> {
        let mut flags = libc::O_CLOEXEC;
        if self.append {
            flags |= libc::O_APPEND;
        }
        if self.truncate {
            flags |= libc::O_TRUNC;
        }
        if self.create_new {
            flags |= libc::O_CREAT | libc::O_EXCL;
        } else if self.create {
            flags |= libc::O_CREAT;
        }

        // Determine access mode
        match (self.read, self.write, self.append) {
            (false, false, false) => flags |= libc::O_RDONLY,
            (true, false, false) => flags |= libc::O_RDONLY,
            (false, true, false) => flags |= libc::O_WRONLY,
            (true, true, false) => flags |= libc::O_RDWR,
            (false, _, true) => flags |= libc::O_WRONLY,
            (true, _, true) => flags |= libc::O_RDWR,
        }

        let zpath = path_to_zbox(path);
        File::open(zpath.as_zstr(), flags, self.mode)
            .map_err(|e| std::io::Error::from_raw_os_error(e.errno as i32))
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self::new()
    }
}

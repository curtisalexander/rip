//! Platform-specific deletion.
//!
//! On Windows we go straight to the Win32 API for maximum speed:
//!   * Open a handle with `DELETE` access (and `FILE_FLAG_BACKUP_SEMANTICS` so
//!     the same call works for directories, `FILE_FLAG_OPEN_REPARSE_POINT` so we
//!     never follow a junction/symlink out of the tree).
//!   * `SetFileInformationByHandle(FileDispositionInfoEx)` with POSIX semantics
//!     so the directory entry vanishes immediately, plus IGNORE_READONLY so
//!     read-only files (hello `.git` packs) die without a separate syscall.
//!   * Address every entry through a `\\?\` verbatim path so deep trees that
//!     exceed the legacy `MAX_PATH` (260 chars) — routine in `node_modules` —
//!     delete without error. `std::fs` does this internally; our raw Win32
//!     calls have to do it by hand.
//!
//! Everywhere else we fall back to `std::fs` with a read-only clearing retry.

use std::io;
use std::path::Path;

#[cfg(windows)]
pub use win::{remove_dir, remove_file};

#[cfg(not(windows))]
pub use portable::{remove_dir, remove_file};

#[cfg(not(windows))]
mod portable {
    use super::*;

    fn clear_readonly(path: &Path) {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            // `set_permissions` follows symlinks, so clearing a *link's*
            // read-only bit would actually chmod whatever it points at —
            // precisely the reach-outside-the-tree we refuse to do elsewhere.
            // Unlinking a symlink never needs its permissions cleared anyway, so
            // leave links untouched.
            if meta.file_type().is_symlink() {
                return;
            }
            let mut perms = meta.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)]
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(_) => {
                clear_readonly(path);
                std::fs::remove_file(path)
            }
        }
    }

    pub fn remove_dir(path: &Path) -> io::Result<()> {
        match std::fs::remove_dir(path) {
            Ok(()) => Ok(()),
            Err(_) => {
                clear_readonly(path);
                std::fs::remove_dir(path)
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use core::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Component, Prefix};

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
        FILE_DISPOSITION_INFO_EX, FILE_DISPOSITION_INFO_EX_FLAGS, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileDispositionInfoEx, OPEN_EXISTING, SetFileInformationByHandle,
    };
    use windows::core::PCWSTR;

    /// Encode `path` as a NUL-terminated wide string carrying the `\\?\`
    /// verbatim prefix, so the call escapes the legacy `MAX_PATH` (260-char)
    /// limit.
    ///
    /// `std::path::absolute` fully qualifies the path lexically — resolving
    /// `.`, `..`, `/`, and drive-relative forms — with no filesystem access, so
    /// it never resolves a trailing symlink into its target and reparse-point
    /// safety is preserved. A verbatim path *must* be fully qualified and
    /// backslash-separated, which is exactly what that gives us.
    fn to_wide(path: &Path) -> Vec<u16> {
        // Falls back to the original path only if the cwd is unavailable, in
        // which case the delete simply fails and is reported like any error.
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let body = abs.as_os_str();

        let wide: Vec<u16> = match abs.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                // Already escaped (`\\?\…`) or a device path (`\\.\…`): as-is.
                Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(..)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_) => body.encode_wide().collect(),
                // Plain UNC `\\server\share…` -> `\\?\UNC\server\share…`
                // (drop one leading backslash, prepend the UNC verbatim prefix).
                Prefix::UNC(..) => r"\\?\UNC"
                    .encode_utf16()
                    .chain(body.encode_wide().skip(1))
                    .collect(),
                // Drive path `C:\…` (the common case) -> `\\?\C:\…`.
                Prefix::Disk(_) => r"\\?\".encode_utf16().chain(body.encode_wide()).collect(),
            },
            // No drive/UNC prefix (couldn't fully qualify): fall back unprefixed.
            _ => body.encode_wide().collect(),
        };

        wide.into_iter().chain(std::iter::once(0)).collect()
    }

    /// Map a Windows API error to `io::Error`. These errors carry a Win32 code
    /// wrapped in a FACILITY_WIN32 `HRESULT` (`0x8007_xxxx`); mask back to the
    /// bare Win32 code so it renders as e.g. "Access is denied." rather than a
    /// raw negative HRESULT with no message.
    fn to_io(e: windows::core::Error) -> io::Error {
        io::Error::from_raw_os_error(e.code().0 & 0xFFFF)
    }

    /// Delete a single entry (file or directory) by handle, POSIX semantics.
    fn delete(path: &Path) -> io::Result<()> {
        let wide = to_wide(path);
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                DELETE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
        }
        .map_err(to_io)?;

        // These flag constants don't implement BitOr in this crate version,
        // so combine the raw bits and wrap.
        let info = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
                FILE_DISPOSITION_FLAG_DELETE.0
                    | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0
                    | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE.0,
            ),
        };

        let result = unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfoEx,
                &info as *const _ as *const c_void,
                core::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        };

        unsafe {
            let _ = CloseHandle(handle);
        }
        result.map_err(to_io)
    }

    pub fn remove_file(path: &Path) -> io::Result<()> {
        delete(path)
    }

    pub fn remove_dir(path: &Path) -> io::Result<()> {
        delete(path)
    }
}

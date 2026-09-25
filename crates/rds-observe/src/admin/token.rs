//! Safe Unix file operations for the supported Linux/macOS targets.
//! Ancestor directories and the local OS identity are trusted; the final entry
//! must be a singly linked, private regular file owned by the effective user.

use std::io::{Read as _, Write as _};
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use super::Error;

/// A 256-bit random credential encoded as exactly 64 lowercase hex characters.
/// Debug/Display are deliberately not implemented.
pub struct Token(Zeroizing<[u8; 64]>);

impl Token {
    /// Read once at startup. Rotation requires restarting the listener.
    pub fn load(path: &Path) -> Result<Self, Error> {
        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|_| Error::TokenFile)?;
        let stat = rustix::fs::fstat(&fd).map_err(|_| Error::TokenFile)?;
        if rustix::fs::FileType::from_raw_mode(stat.st_mode) != rustix::fs::FileType::RegularFile
            || stat.st_uid != rustix::process::geteuid().as_raw()
            || stat.st_nlink != 1
            || stat.st_mode & 0o177 != 0
            || stat.st_mode & 0o400 == 0
        {
            return Err(Error::TokenFile);
        }
        let mut file = std::fs::File::from(fd);
        let mut bytes = Zeroizing::new(Vec::new());
        (&mut file)
            .take(66)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::TokenFile)?;
        let value = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        if value.len() != 64
            || !value
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Err(Error::TokenFile);
        }
        let mut token = Zeroizing::new([0; 64]);
        token.copy_from_slice(value);
        Ok(Self(token))
    }

    pub(super) fn accepts(&self, value: &[u8]) -> bool {
        // Length and the authentication scheme are public; compare only the
        // fixed-length credential with the existing constant-time primitive.
        let Some(scheme) = value.get(..7) else {
            return false;
        };
        scheme[..6].eq_ignore_ascii_case(b"Bearer")
            && scheme[6] == b' '
            && bool::from(self.0.as_slice().ct_eq(&value[7..]))
    }

    /// Create a new private credential without overwriting any existing entry.
    /// No endpoint identity is created and no secret is printed or logged.
    pub fn create(path: &Path) -> Result<(), Error> {
        let entropy = Zeroizing::new(rand::random::<[u8; 32]>());
        let mut bytes = Zeroizing::new([0u8; 64]);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (i, value) in entropy.iter().enumerate() {
            bytes[2 * i] = HEX[(value >> 4) as usize];
            bytes[2 * i + 1] = HEX[(value & 15) as usize];
        }
        let fd = rustix::fs::open(
            path,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|_| Error::TokenFile)?;
        let mut file = std::fs::File::from(fd);
        file.write_all(bytes.as_slice())
            .map_err(|_| Error::TokenFile)?;
        file.sync_all().map_err(|_| Error::TokenFile)?;
        // On failure preserve the new file for explicit operator inspection;
        // never unlink a pathname that might already name a replacement.
        Ok(())
    }
}

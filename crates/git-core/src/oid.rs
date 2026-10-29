use std::fmt;

/// A git object id. Supports SHA-1 (20 bytes) and SHA-256 (32 bytes)
/// repositories. `Copy`, so it is cheap to use as a graph key.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Oid {
    bytes: [u8; 32],
    len: u8,
}

impl Oid {
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.as_bytes();
        if hex.len() != 40 && hex.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, pair) in hex.chunks_exact(2).enumerate() {
            bytes[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Some(Self {
            bytes,
            len: (hex.len() / 2) as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    /// Abbreviated form for display, like `git log --oneline`.
    pub fn short(&self) -> String {
        let mut s = self.to_string();
        s.truncate(8);
        s
    }
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.as_bytes() {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Oid({})", self.short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_sha1_and_sha256() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(Oid::from_hex(sha1).unwrap().to_string(), sha1);
        let sha256 = "ab".repeat(32);
        assert_eq!(Oid::from_hex(&sha256).unwrap().to_string(), sha256);
        assert_eq!(Oid::from_hex(sha1).unwrap().short(), "01234567");
    }

    #[test]
    fn rejects_garbage() {
        assert!(Oid::from_hex("xyz").is_none());
        assert!(Oid::from_hex(&"g".repeat(40)).is_none());
    }
}

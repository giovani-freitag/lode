use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    hex(&Sha1::digest(bytes))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn sha512_hex(bytes: &[u8]) -> String {
    hex(&Sha512::digest(bytes))
}

/// MD5 of `bytes`. Weak by modern standards, but it's the only hash CurseForge exposes for some
/// files, so verifying against it still catches truncation/corruption — the threat a download
/// check exists to catch.
pub fn md5_hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}

/// Hash `bytes` with the named algorithm, or `None` if the format isn't one we verify. Used to
/// check a downloaded jar against the provider-native hash stored in the lock. `md5` is included
/// because CurseForge locks it as the only hash for some files.
pub fn hash_by_format(bytes: &[u8], format: &str) -> Option<String> {
    match format {
        "sha1" => Some(sha1_hex(bytes)),
        "sha256" => Some(sha256_hex(bytes)),
        "sha512" => Some(sha512_hex(bytes)),
        "md5" => Some(md5_hex(bytes)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_matches_known_vectors() {
        // RFC 1321 test vectors — proves the digest wiring is correct, not just self-consistent.
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn hash_by_format_verifies_md5() {
        assert_eq!(
            hash_by_format(b"abc", "md5").as_deref(),
            Some("900150983cd24fb0d6963f7d28e17f72")
        );
    }
}

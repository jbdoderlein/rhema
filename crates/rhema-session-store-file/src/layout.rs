//! The workspace directory name.

use std::path::Path;

/// The workspace directory name: the absolute workspace path with every
/// byte outside `[-._0-9A-Za-z]` percent-encoded.
///
/// The encoding is frozen. A directory written by an older build must
/// keep resolving, so the allowed set and the uppercase hex digits are
/// part of the store's contract, not an implementation detail.
pub fn encode_workspace(workspace: &Path) -> String {
    let mut out = String::new();
    for byte in workspace.to_string_lossy().bytes() {
        match byte {
            b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_paths_never_share_a_name() {
        assert_ne!(
            encode_workspace(Path::new("/a/b")),
            encode_workspace(Path::new("/a_b"))
        );
        assert_eq!(encode_workspace(Path::new("/a/b")), "%2Fa%2Fb");
    }

    #[test]
    fn the_alphabet_stays_readable() {
        assert_eq!(
            encode_workspace(Path::new("/tmp/my-proj.v2")),
            "%2Ftmp%2Fmy-proj.v2"
        );
    }
}

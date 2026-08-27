//! `file://` URI decoding for the freedesktop interfaces.
//!
//! The org.freedesktop.FileManager1 methods take URIs, not paths, and callers
//! percent-encode anything outside the unreserved set -- Chromium sends
//! `file:///home/u/Big%20Report.pdf` for a name with a space in it. Decoding is
//! a few lines and is kept here, next to its tests, rather than pulling in a
//! URI crate for one string transformation.

use std::path::PathBuf;

/// Decode a `file://` URI into a local path.
///
/// Accepts the empty and `localhost` authorities, which are the two spellings
/// the spec allows for a local file, and tolerates a bare path so a caller that
/// passes `/home/u/x` instead of a URI still gets what it meant. Returns `None`
/// for any other scheme or authority -- a `sftp://` URI names a real file, but
/// not one this function can turn into a path.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let uri = uri.trim();

    let rest = if let Some(after_scheme) = strip_file_scheme(uri) {
        // Split the authority from the path. The path always begins at the
        // first '/', so no '/' at all means there is no path to speak of.
        match after_scheme.find('/') {
            Some(0) => after_scheme,
            Some(slash) => {
                let (authority, path) = after_scheme.split_at(slash);
                if authority.eq_ignore_ascii_case("localhost") {
                    path
                } else {
                    return None;
                }
            }
            None => return None,
        }
    } else if uri.starts_with('/') {
        uri
    } else {
        return None;
    };

    let decoded = percent_decode(rest)?;
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.is_empty() {
        None
    } else {
        Some(PathBuf::from(decoded))
    }
}

/// Case-insensitive `file://` prefix strip; schemes are not case-sensitive.
fn strip_file_scheme(uri: &str) -> Option<&str> {
    const PREFIX: &str = "file://";
    if uri.len() >= PREFIX.len() && uri[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        Some(&uri[PREFIX.len()..])
    } else {
        None
    }
}

/// Percent-decode to bytes. Decoding to bytes rather than to a string matters:
/// a path is bytes on Linux, and a filename can hold a sequence that is not
/// valid UTF-8. Callers decide what to do with that; here it simply round-trips
/// until the final conversion.
fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            // A trailing '%' or one short of two digits is malformed, not a
            // literal -- decoding it leniently would invent a path.
            if i + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_digit(bytes[i + 1])?;
            let lo = hex_digit(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_file_uri() {
        assert_eq!(
            file_uri_to_path("file:///home/user/Documents"),
            Some(PathBuf::from("/home/user/Documents"))
        );
    }

    #[test]
    fn percent_encoded_space() {
        assert_eq!(
            file_uri_to_path("file:///home/user/Big%20Report.pdf"),
            Some(PathBuf::from("/home/user/Big Report.pdf"))
        );
    }

    #[test]
    fn percent_encoded_multibyte() {
        // 'é' is two bytes in UTF-8, so it arrives as two escapes.
        assert_eq!(
            file_uri_to_path("file:///tmp/caf%C3%A9.txt"),
            Some(PathBuf::from("/tmp/café.txt"))
        );
    }

    #[test]
    fn localhost_authority_is_accepted() {
        assert_eq!(
            file_uri_to_path("file://localhost/etc/hosts"),
            Some(PathBuf::from("/etc/hosts"))
        );
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(
            file_uri_to_path("FILE:///tmp/x"),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn bare_path_is_tolerated() {
        assert_eq!(
            file_uri_to_path("/home/user/x"),
            Some(PathBuf::from("/home/user/x"))
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            file_uri_to_path("  file:///tmp/x  "),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn remote_authority_is_rejected() {
        assert_eq!(file_uri_to_path("file://example.com/share/x"), None);
    }

    #[test]
    fn other_schemes_are_rejected() {
        assert_eq!(file_uri_to_path("sftp://host/home/user"), None);
        assert_eq!(file_uri_to_path("https://example.com/"), None);
        assert_eq!(file_uri_to_path("trash:///"), None);
    }

    #[test]
    fn malformed_escapes_are_rejected() {
        assert_eq!(file_uri_to_path("file:///tmp/%"), None);
        assert_eq!(file_uri_to_path("file:///tmp/%2"), None);
        assert_eq!(file_uri_to_path("file:///tmp/%zz"), None);
    }

    #[test]
    fn empty_and_authority_only_are_rejected() {
        assert_eq!(file_uri_to_path(""), None);
        assert_eq!(file_uri_to_path("file://"), None);
        assert_eq!(file_uri_to_path("file://localhost"), None);
    }

    #[test]
    fn invalid_utf8_is_rejected_rather_than_mangled() {
        // 0xFF is not valid UTF-8; better to decline than to substitute U+FFFD
        // and act on a path that is not the one the caller named.
        assert_eq!(file_uri_to_path("file:///tmp/%FF"), None);
    }
}

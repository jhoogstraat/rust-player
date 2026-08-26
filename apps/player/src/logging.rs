//! Application-owned diagnostic logging: rotating files under the data root,
//! one redaction pass before anything is written. The fork's console logger
//! is never installed.

use std::path::{Path, PathBuf};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
const KEEP_ROTATIONS: usize = 3;

/// Install the logger and return the active log path.
pub fn init(data_root: &Path) -> anyhow::Result<PathBuf> {
    let dir = data_root.join("logs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("player.log");
    rotate(&path)?;

    let dispatch = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] [{}] {}",
                timestamp(),
                record.level(),
                record.target(),
                redact(&message.to_string()),
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(fern::log_file(&path)?);
    dispatch.apply()?;
    Ok(path)
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Shift player.log → player.log.1 → … → player.log.N when it grows past
/// [`MAX_LOG_BYTES`].
fn rotate(path: &Path) -> std::io::Result<()> {
    if !path.exists() || std::fs::metadata(path)?.len() < MAX_LOG_BYTES {
        return Ok(());
    }
    for i in (1..KEEP_ROTATIONS).rev() {
        let from = rotated(path, i);
        if from.exists() {
            let _ = std::fs::rename(from, rotated(path, i + 1));
        }
    }
    let _ = std::fs::rename(path, rotated(path, 1));
    Ok(())
}

fn rotated(path: &Path, n: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), n))
}

/// Keys whose following value is a secret: bearer/refresh/access tokens,
/// client secrets, OAuth `code=` parameters, cookies.
const SECRET_KEYS: [&str; 5] = [
    "code=",
    "refresh_token",
    "access_token",
    "client_secret",
    "cookie",
];

/// Strip secrets from one log line before it is written. Authorization
/// headers lose everything after the colon; every [`SECRET_KEYS`] value is
/// replaced. Unit-tested below.
pub fn redact(line: &str) -> String {
    // The needles are ASCII; ASCII lowercasing keeps byte offsets aligned
    // with `line` (Unicode lowercasing can change byte lengths).
    let lower = line.to_ascii_lowercase();

    // Authorization headers: everything after the colon is secret. Keys in
    // the part before it are still scanned below.
    let auth_cut = lower
        .find("authorization")
        .and_then(|pos| line[pos..].find(':').map(|colon| pos + colon + 1));
    let (line, auth_tail) = match auth_cut {
        Some(cut) => (&line[..cut], " [redacted]"),
        None => (line, ""),
    };
    let lower = &lower[..line.len()];

    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    while let Some((key_at, key)) = next_secret_key(lower, cursor) {
        // The value starts past the key and any '=', ':', quotes, or spaces …
        let mut start = key_at + key.len();
        while start < bytes.len() && matches!(bytes[start], b'"' | b'\'' | b'=' | b':' | b' ') {
            start += 1;
        }
        // … and ends at whitespace or a structural delimiter. Both bounds sit
        // on ASCII bytes (or the end), so they are char boundaries.
        let mut end = start;
        while end < bytes.len()
            && !matches!(
                bytes[end],
                b' ' | b'&' | b',' | b')' | b']' | b'}' | b'"' | b'\'' | b';'
            )
        {
            end += 1;
        }
        out.push_str(&line[cursor..start]);
        if end > start {
            out.push_str("[redacted]");
        }
        cursor = end;
    }
    out.push_str(&line[cursor..]);
    out.push_str(auth_tail);
    out
}

/// The earliest secret key at or after `from`.
fn next_secret_key(lower: &str, from: usize) -> Option<(usize, &'static str)> {
    SECRET_KEYS
        .iter()
        .filter_map(|key| lower[from..].find(key).map(|at| (from + at, *key)))
        .min_by_key(|(at, _)| *at)
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn oauth_code_parameter_is_redacted() {
        let out = redact("GET /callback?code=4/0Ab_32secret&state=xyz");
        assert_eq!(out, "GET /callback?code=[redacted]&state=xyz");
    }

    #[test]
    fn refresh_and_access_tokens_are_redacted() {
        let out = redact(r#"{"refresh_token":"AQD-secret-1","access_token":"BQC-secret-2"}"#);
        assert_eq!(
            out,
            r#"{"refresh_token":"[redacted]","access_token":"[redacted]"}"#
        );
    }

    #[test]
    fn every_occurrence_is_redacted() {
        let out = redact("first code=aaa then code=bbb and Cookie: sid=ccc");
        assert_eq!(
            out,
            "first code=[redacted] then code=[redacted] and Cookie: [redacted]"
        );
    }

    #[test]
    fn authorization_header_is_redacted() {
        let out = redact("authorization: Bearer BQAAAverysecrettoken123");
        assert_eq!(out, "authorization: [redacted]");
    }

    #[test]
    fn client_secrets_are_redacted() {
        let out = redact("client_secret=deadbeefcafe");
        assert_eq!(out, "client_secret=[redacted]");
    }

    #[test]
    fn keys_before_an_authorization_header_are_still_redacted() {
        let out = redact("access_token=abc123 Authorization: Bearer x");
        assert_eq!(out, "access_token=[redacted] Authorization: [redacted]");
    }

    #[test]
    fn non_ascii_text_keeps_offsets_aligned() {
        // 'İ' lowercases to two chars in Unicode; byte offsets must not shift.
        let out = redact("İstanbul access_token=abc123 done");
        assert_eq!(out, "İstanbul access_token=[redacted] done");
        let out = redact("İstanbul — authorization: Bearer x");
        assert_eq!(out, "İstanbul — authorization: [redacted]");
    }

    #[test]
    fn empty_values_and_ordinary_lines_pass_through() {
        assert_eq!(redact("code=&state=xyz"), "code=&state=xyz");
        let line = "playback tick position=42000 playing=true";
        assert_eq!(redact(line), line);
    }
}

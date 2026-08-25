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
    let now = chrono::Local::now();
    now.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Shift player.log → player.log.1 → … → player.log.N when it grows past
/// [`MAX_LOG_BYTES`].
fn rotate(path: &Path) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return Ok(());
    }
    if !path.exists() || std::fs::metadata(path)?.len() < MAX_LOG_BYTES {
        return Ok(());
    }
    for i in (1..KEEP_ROTATIONS).rev() {
        let from = rotated(path, i);
        let to = rotated(path, i + 1);
        if from.exists() {
            let _ = std::fs::rename(from, to);
        }
    }
    if path.exists() {
        let _ = std::fs::rename(path, rotated(path, 1));
    }
    Ok(())
}

fn rotated(path: &Path, n: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", path.display(), n))
}

/// Strip secrets from one log line before it is written: bearer tokens,
/// refresh/access tokens, client secrets, OAuth `code=` parameters, cookies,
/// and authorization headers. Tested in `tests/redaction.rs`.
pub fn redact(line: &str) -> String {
    let lower = line.to_lowercase();

    // Authorization headers: everything after the colon is secret.
    if let Some(pos) = lower.find("authorization") {
        if let Some(colon) = line[pos..].find(':') {
            let cut = pos + colon + 1;
            return format!("{} [redacted]", &line[..cut]);
        }
    }

    let mut out = line.to_string();
    for needle in [
        "code=",
        "refresh_token",
        "access_token",
        "client_secret",
        "cookie",
    ] {
        while let Some(rel) = out.to_lowercase().find(needle) {
            // Find where the value starts: past '=', '"', ':', or whitespace run.
            let start = rel + needle.len();
            let bytes = out.as_bytes();
            let mut value_start = start;
            while value_start < bytes.len()
                && matches!(bytes[value_start], b'"' | b'\'' | b'=' | b':' | b' ')
            {
                value_start += 1;
            }
            if value_start >= bytes.len() {
                break;
            }
            // Values end at whitespace, '&', quote, comma, or closing bracket.
            let mut value_end = value_start;
            while value_end < bytes.len()
                && !matches!(
                    bytes[value_start],
                    b'"' | b'\'' // quoted value: ends at matching quote handled below
                )
                && !matches!(
                    bytes[value_end],
                    b' ' | b'&' | b',' | b')' | b']' | b'}' | b'"' | b';'
                )
            {
                value_end += 1;
            }
            // Quoted values ("token"): consume trailing quote too.
            if value_end < bytes.len()
                && bytes[value_start - 1] == b'"'
                && bytes.get(value_end) == Some(&b'"')
            {
                value_end += 1;
            }
            out = format!(
                "{}[redacted]{}",
                &out[..value_start],
                &out[value_end.min(out.len())..]
            );
            break; // rescan finds further occurrences
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn oauth_code_parameter_is_redacted() {
        let out = redact("GET /callback?code=4/0Ab_32secret&state=xyz");
        assert!(!out.contains("4/0Ab_32secret"));
        assert!(out.contains("code=[redacted]"));
        assert!(out.contains("state=xyz"));
    }

    #[test]
    fn refresh_and_access_tokens_are_redacted() {
        let out = redact(r#"{"refresh_token":"AQD-secret-1","access_token":"BQC-secret-2"}"#);
        assert!(!out.contains("AQD-secret-1"));
        assert!(!out.contains("BQC-secret-2"));
        assert!(out.matches("[redacted]").count() >= 2);
    }

    #[test]
    fn authorization_header_is_redacted() {
        let out = redact("authorization: Bearer BQAAAverysecrettoken123");
        assert!(!out.contains("BQAAAverysecrettoken123"));
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn client_secrets_are_redacted() {
        let out = redact("client_secret=deadbeefcafe");
        assert!(!out.contains("deadbeefcafe"));
    }

    #[test]
    fn ordinary_lines_pass_through() {
        let line = "playback tick position=42000 playing=true";
        assert_eq!(redact(line), line);
    }
}

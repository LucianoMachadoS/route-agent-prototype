//! Small, reusable "append a CSV row, writing the header once" helper -
//! used by both `bench`'s run log (`docs/runs.csv`) and `monitor`'s
//! decision log (`docs/decisions.csv`), instead of duplicating the same
//! file-append-with-header logic twice. This used to be private to
//! `bench.rs`; pulled out here when `monitor` needed the same thing.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use crate::error::Result;

/// Quotes a CSV field if it contains a comma, quote or newline.
pub fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn fmt_opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.1}"))
        .unwrap_or_else(|| "-".to_string())
}

/// Appends one line to `path`, creating it (and its parent directory) and
/// writing `header` first if it doesn't exist yet.
pub fn append_line(path: &Path, header: &str, line: &str) -> Result<()> {
    let is_new = !path.is_file();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if is_new {
        writeln!(file, "{header}")?;
    }
    writeln!(file, "{line}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn csv_field_quotes_commas_and_quotes() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn append_line_writes_header_once() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("log.csv");
        append_line(&path, "h1,h2", "1,2").unwrap();
        append_line(&path, "h1,h2", "3,4").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines, vec!["h1,h2", "1,2", "3,4"]);
    }
}

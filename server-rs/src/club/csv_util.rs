//! Minimal CSV parse/write for roster bulk import (no extra crates).

const MAX_CSV_BYTES: usize = 256 * 1024;
pub const MAX_CSV_ROWS: usize = 500;

#[derive(Debug, Clone)]
pub struct CsvRow {
    pub fields: Vec<String>,
    pub line: usize,
}

#[derive(Debug)]
pub enum CsvParseError {
    TooLarge,
    TooManyRows,
    Empty,
}

pub fn parse_csv_bytes(body: &[u8]) -> Result<Vec<CsvRow>, CsvParseError> {
    if body.len() > MAX_CSV_BYTES {
        return Err(CsvParseError::TooLarge);
    }
    let text = std::str::from_utf8(body).unwrap_or("");
    let mut rows = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let line_no = line_no + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if rows.len() >= MAX_CSV_ROWS {
            return Err(CsvParseError::TooManyRows);
        }
        rows.push(CsvRow {
            fields: split_csv_line(trimmed),
            line: line_no,
        });
    }
    if rows.is_empty() {
        return Err(CsvParseError::Empty);
    }
    Ok(rows)
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// Prefix dangerous spreadsheet formula starters for safe CSV download.
#[allow(dead_code)]
pub fn csv_escape_field(s: &str) -> String {
    let needs_quote = s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r');
    let mut v = s.to_string();
    if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        v.insert(0, '\'');
    }
    if needs_quote || v.contains('\'') && v.starts_with('\'') {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

#[allow(dead_code)]
pub fn write_csv_row(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|f| csv_escape_field(f))
        .collect::<Vec<_>>()
        .join(",")
}

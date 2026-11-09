use crate::data::{DataValue, Document, ExactNumber};
use std::fmt;

pub const MAX_TABLE_ROWS: usize = 1_000_000;
pub const MAX_COLUMNS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonError {
    UnexpectedEof,
    EmptyInput,
    MissingHeader,
    DuplicateColumn(String),
    MaxColumnsExceeded(usize),
    MaxRowsExceeded(usize),
    ColumnMismatch { expected: usize, found: usize, row: usize },
    InvalidScalar(String, usize),
}

impl fmt::Display for ToonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToonError::UnexpectedEof => write!(f, "Unexpected end of TOON input"),
            ToonError::EmptyInput => write!(f, "TOON input is empty"),
            ToonError::MissingHeader => write!(f, "Missing TOON header row"),
            ToonError::DuplicateColumn(c) => write!(f, "Duplicate column name '{}' in TOON header", c),
            ToonError::MaxColumnsExceeded(c) => write!(f, "Columns {} exceeded maximum of {}", c, MAX_COLUMNS),
            ToonError::MaxRowsExceeded(r) => write!(f, "Rows {} exceeded maximum of {}", r, MAX_TABLE_ROWS),
            ToonError::ColumnMismatch { expected, found, row } => {
                write!(f, "Row {} has {} values, expected {}", row, found, expected)
            }
            ToonError::InvalidScalar(s, row) => write!(f, "Invalid scalar '{}' at row {}", s, row),
        }
    }
}

impl std::error::Error for ToonError {}

/// Compact row/column tabular frontend for TOON
#[derive(Debug, Clone, PartialEq)]
pub struct ToonTable {
    columns: Vec<String>,
    rows: Vec<Document>,
}

impl ToonTable {
    pub fn new(columns: Vec<String>, rows: Vec<Document>) -> Result<Self, ToonError> {
        if columns.is_empty() {
            return Err(ToonError::MissingHeader);
        }
        if columns.len() > MAX_COLUMNS {
            return Err(ToonError::MaxColumnsExceeded(columns.len()));
        }
        // Verify unique columns
        for (i, col) in columns.iter().enumerate() {
            for other in &columns[i + 1..] {
                if col == other {
                    return Err(ToonError::DuplicateColumn(col.clone()));
                }
            }
        }
        if rows.len() > MAX_TABLE_ROWS {
            return Err(ToonError::MaxRowsExceeded(rows.len()));
        }
        // Verify each row has matching columns
        for (idx, row) in rows.iter().enumerate() {
            if row.len() != columns.len() {
                return Err(ToonError::ColumnMismatch {
                    expected: columns.len(),
                    found: row.len(),
                    row: idx + 1,
                });
            }
        }
        Ok(Self { columns, rows })
    }

    /// Construct a table from columnar data (column names and corresponding column value vectors)
    pub fn from_columns(columns: Vec<String>, columns_data: Vec<Vec<DataValue>>) -> Result<Self, ToonError> {
        if columns.len() != columns_data.len() {
            return Err(ToonError::ColumnMismatch {
                expected: columns.len(),
                found: columns_data.len(),
                row: 0,
            });
        }
        if columns.is_empty() {
            return Err(ToonError::MissingHeader);
        }
        let row_count = columns_data[0].len();
        for (idx, col_data) in columns_data.iter().enumerate() {
            if col_data.len() != row_count {
                return Err(ToonError::ColumnMismatch {
                    expected: row_count,
                    found: col_data.len(),
                    row: idx + 1,
                });
            }
        }

        let mut rows = Vec::with_capacity(row_count);
        for r in 0..row_count {
            let mut doc = Document::new();
            for (c, col_name) in columns.iter().enumerate() {
                let _ = doc.try_insert(col_name.clone(), columns_data[c][r].clone());
            }
            rows.push(doc);
        }

        Self::new(columns, rows)
    }

    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get_row(&self, index: usize) -> Option<&Document> {
        self.rows.get(index)
    }

    pub fn get_cell(&self, row: usize, col: &str) -> Option<&DataValue> {
        self.rows.get(row).and_then(|r| r.get(col))
    }

    /// Columnar slice accessor
    pub fn column(&self, col: &str) -> Result<Vec<&DataValue>, ToonError> {
        if !self.columns.iter().any(|c| c == col) {
            return Err(ToonError::InvalidScalar(format!("unknown column: {}", col), 0));
        }
        let mut vals = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            if let Some(val) = row.get(col) {
                vals.push(val);
            } else {
                vals.push(&DataValue::Null);
            }
        }
        Ok(vals)
    }

    pub fn to_data_value(&self) -> DataValue {
        DataValue::Array(self.rows.iter().cloned().map(DataValue::Document).collect())
    }

    pub fn to_documents(&self) -> &[Document] {
        &self.rows
    }

    pub fn decode(input: &str) -> Result<Self, ToonError> {
        ToonCodec::decode_table(input)
    }

    pub fn encode_compact(&self) -> Result<String, ToonError> {
        ToonCodec::encode_table(self)
    }
}

pub struct ToonCodec;

impl ToonCodec {
    pub fn decode(input: &str) -> Result<DataValue, ToonError> {
        let lines: Vec<&str> = input.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        if lines.is_empty() {
            return Err(ToonError::EmptyInput);
        }

        // Header line: [col1, col2, ...] or col1 | col2 | ... or col1, col2, ...
        let header_line = lines[0];
        let columns = parse_row_cells(header_line);
        if columns.is_empty() {
            return Err(ToonError::MissingHeader);
        }
        if columns.len() > MAX_COLUMNS {
            return Err(ToonError::MaxColumnsExceeded(columns.len()));
        }

        // Verify unique columns (Gate-SCHEMA-01)
        for (i, col) in columns.iter().enumerate() {
            for other in &columns[i + 1..] {
                if col == other {
                    return Err(ToonError::DuplicateColumn(col.clone()));
                }
            }
        }

        let mut rows = Vec::new();
        for (row_idx, &line) in lines[1..].iter().enumerate() {
            if rows.len() >= MAX_TABLE_ROWS {
                return Err(ToonError::MaxRowsExceeded(rows.len()));
            }

            let cells = parse_row_cells(line);
            if cells.len() != columns.len() {
                return Err(ToonError::ColumnMismatch {
                    expected: columns.len(),
                    found: cells.len(),
                    row: row_idx + 1,
                });
            }

            let mut doc = Document::new();
            for (col_idx, val_str) in cells.into_iter().enumerate() {
                let val = parse_scalar(&val_str, row_idx + 1)?;
                let col_name = &columns[col_idx];
                let _ = doc.try_insert(col_name.clone(), val);
            }
            rows.push(DataValue::Document(doc));
        }

        Ok(DataValue::Array(rows))
    }

    pub fn decode_table(input: &str) -> Result<ToonTable, ToonError> {
        let val = Self::decode(input)?;
        match val {
            DataValue::Array(rows) => {
                let mut docs = Vec::with_capacity(rows.len());
                let mut columns = Vec::new();
                for (idx, r) in rows.into_iter().enumerate() {
                    match r {
                        DataValue::Document(d) => {
                            if idx == 0 {
                                columns = d.keys().cloned().collect();
                            }
                            docs.push(d);
                        }
                        _ => return Err(ToonError::InvalidScalar("Row is not a document".to_string(), idx + 1)),
                    }
                }
                if columns.is_empty() && !docs.is_empty() {
                    columns = docs[0].keys().cloned().collect();
                }
                ToonTable::new(columns, docs)
            }
            _ => Err(ToonError::EmptyInput),
        }
    }

    pub fn encode(value: &DataValue) -> Result<String, ToonError> {
        match value {
            DataValue::Array(rows) => {
                if rows.is_empty() {
                    return Ok("[]\n".to_string());
                }

                // Extract columns from first row
                let first_doc = match &rows[0] {
                    DataValue::Document(d) => d,
                    _ => return Ok(String::new()),
                };

                let columns: Vec<String> = first_doc.iter().map(|(k, _)| k.clone()).collect();
                let mut out = String::new();

                // Header
                out.push('|');
                for col in &columns {
                    out.push(' ');
                    out.push_str(col);
                    out.push(' ');
                    out.push('|');
                }
                out.push('\n');

                // Rows
                for row_val in rows {
                    if let DataValue::Document(doc) = row_val {
                        out.push('|');
                        for col in &columns {
                            out.push(' ');
                            if let Some(val) = doc.get(col) {
                                out.push_str(&format_scalar(val));
                            } else {
                                out.push_str("null");
                            }
                            out.push(' ');
                            out.push('|');
                        }
                        out.push('\n');
                    }
                }

                Ok(out)
            }
            _ => Ok(String::new()),
        }
    }

    pub fn encode_table(table: &ToonTable) -> Result<String, ToonError> {
        let val = table.to_data_value();
        Self::encode(&val)
    }
}

fn parse_row_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let content = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        &trimmed[1..trimmed.len() - 1]
    } else if trimmed.starts_with('|') && trimmed.ends_with('|') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    let delimiter = if content.contains('|') {
        '|'
    } else if content.contains('\t') {
        '\t'
    } else {
        ','
    };

    content
        .split(delimiter)
        .map(|s| strip_quotes(s.trim()))
        .collect()
}

fn strip_quotes(s: &str) -> String {
    let trimmed = s.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        if trimmed.len() >= 2 {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn parse_scalar(s: &str, row: usize) -> Result<DataValue, ToonError> {
    let clean = strip_quotes(s);
    let is_quoted = (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''));
    if is_quoted {
        return Ok(DataValue::String(clean));
    }
    match clean.as_str() {
        "null" | "~" => Ok(DataValue::Null),
        "true" | "TRUE" => Ok(DataValue::Bool(true)),
        "false" | "FALSE" => Ok(DataValue::Bool(false)),
        other => {
            let is_integer = {
                let trimmed = other.trim();
                let without_sign = if let Some(stripped) = trimmed.strip_prefix('+').or_else(|| trimmed.strip_prefix('-')) {
                    stripped
                } else {
                    trimmed
                };
                !without_sign.is_empty() && without_sign.chars().all(|c| c.is_ascii_digit())
            };

            if is_integer {
                if let Ok(i) = other.parse::<i64>() {
                    Ok(DataValue::Number(ExactNumber::Int(i)))
                } else {
                    Err(ToonError::InvalidScalar(other.to_string(), row))
                }
            } else if let Ok(f) = other.parse::<f64>() {
                if !f.is_finite() {
                    return Err(ToonError::InvalidScalar(other.to_string(), row));
                }
                Ok(DataValue::Number(ExactNumber::Float(f)))
            } else {
                let looks_numeric = other.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-');
                if looks_numeric {
                    return Err(ToonError::InvalidScalar(other.to_string(), row));
                }
                Ok(DataValue::String(clean))
            }
        }
    }
}

fn format_scalar(v: &DataValue) -> String {
    match v {
        DataValue::Null => "null".to_string(),
        DataValue::Bool(b) => b.to_string(),
        DataValue::Number(ExactNumber::Int(i)) => i.to_string(),
        DataValue::Number(ExactNumber::UInt(u)) => u.to_string(),
        DataValue::Number(ExactNumber::Float(f)) => {
            if f.fract() == 0.0 && !f.to_string().contains('e') && !f.to_string().contains('E') {
                format!("{:.1}", f)
            } else {
                f.to_string()
            }
        }
        DataValue::Number(ExactNumber::Decimal128(d)) => d.to_string(),
        DataValue::String(s) => {
            if s.contains(' ') || s.contains('|') || s.contains(',') {
                format!("\"{}\"", s)
            } else {
                s.clone()
            }
        }
        _ => format!("{:?}", v),
    }
}

pub fn from_str(input: &str) -> Result<DataValue, ToonError> {
    ToonCodec::decode(input)
}

pub fn to_string(value: &DataValue) -> Result<String, ToonError> {
    ToonCodec::encode(value)
}

pub fn decode_table(input: &str) -> Result<ToonTable, ToonError> {
    ToonCodec::decode_table(input)
}

pub fn encode_table(table: &ToonTable) -> Result<String, ToonError> {
    ToonCodec::encode_table(table)
}


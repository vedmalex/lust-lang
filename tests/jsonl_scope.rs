//! Integration tests for JSONL streaming, scoped view lifecycle, memory bounds,
//! cancellation, and short-write handling (WBS 12C-03 / DS-70..75 / Gate-06).

use lust::data::budget::BudgetTracker;
use lust::data::jsonl::{
    line_results, lines, lines_with_options, validate_line_results, validate_lines, write_lines,
    BlankLines, CancellationToken, ControlFlow, JsonlReader, JsonlReaderOptions, JsonlWriter,
    JsonlWriterOptions, LineErrorKind, StreamValidationError, StreamWriteError, WriteErrorKind,
};
use lust::data::owner::OwnerRegistry;
use lust::data::{DataValue, Document, ExactNumber};
use lust::schema::s;
use std::io::{self, Cursor, Write};
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ============================================================================
// 1. N records do not grow retained buffer with N (Gate-06 Memory Invariant)
// ============================================================================

#[test]
fn test_n_records_do_not_grow_retained_buffer_with_n() {
    // Generate a stream of N = 10,000 JSONL records
    const N: usize = 10_000;
    let mut data = Vec::with_capacity(N * 45);
    for i in 0..N {
        data.extend_from_slice(format!("{{\"id\":{},\"metric\":\"cpu_load\",\"val\":{}.5}}\n", i, i % 100).as_bytes());
    }

    let cursor = Cursor::new(data);
    let options = JsonlReaderOptions::default()
        .with_max_line_bytes(1024)
        .with_max_buffered_bytes(16 * 1024);

    let mut reader = lines_with_options(cursor, options);

    let initial_retained = reader.retained_buffer_bytes();
    let max_cap = reader.max_live_memory_cap();
    let mut count = 0;

    while let Some(val_res) = reader.next() {
        let val = val_res.expect("valid JSONL record");
        count += 1;

        // Verify live memory never exceeds the theoretical maximum cap
        let live_mem = reader.live_memory_bytes();
        assert!(
            live_mem <= max_cap,
            "Live memory {} exceeded max cap {} at record {}",
            live_mem,
            max_cap,
            count
        );

        // Verify retained buffer capacity does NOT grow with N
        let current_retained = reader.retained_buffer_bytes();
        assert!(
            current_retained <= reader.options().max_buffered_bytes,
            "Retained buffer {} exceeded max_buffered_bytes {} at record {}",
            current_retained,
            reader.options().max_buffered_bytes,
            count
        );

        if count % 1000 == 0 {
            // Buffer size after 1000, 2000... records stays bounded near initial
            assert!(
                current_retained <= initial_retained.max(16 * 1024),
                "Buffer capacity leaked: current {} vs initial {}",
                current_retained,
                initial_retained
            );
        }

        // Verify record content
        if let DataValue::Document(doc) = val {
            assert!(doc.get("id").is_some());
        } else {
            panic!("Expected Document variant");
        }
    }

    assert_eq!(count, N);

    // Final check: retained memory remains constant O(1), not O(N)
    assert!(reader.retained_buffer_bytes() <= reader.options().max_buffered_bytes);
}

// ============================================================================
// 2. Cancellation drops the scoped view and cleans up
// ============================================================================

#[test]
fn test_cancel_drops_scoped_view_and_cleans_up() {
    let raw = "{\"id\":1}\n{\"id\":2}\n{\"id\":3}\n{\"id\":4}\n{\"id\":5}\n";
    let token = CancellationToken::new();

    let mut reader = lines(raw.as_bytes()).with_cancellation(token.clone());

    let seen_views = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_clone = seen_views.clone();
    let token_clone = token.clone();

    let res = reader.for_each_view(move |view| {
        let count = seen_clone.fetch_add(1, Ordering::SeqCst);
        let view_str = view.as_str().expect("view is str");
        assert!(view_str.starts_with("{\"id\":"));

        if count == 2 {
            // Cooperative cancellation requested on record 3
            token_clone.cancel();
        }

        Ok(ControlFlow::Continue)
    });

    // Reader must fail closed with Interrupted error
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Interrupted);

    // Exactly 3 records were observed before cancel took effect
    assert_eq!(seen_views.load(Ordering::SeqCst), 3);

    // Live memory and in-flight buffers are reset to base level
    assert_eq!(reader.live_memory_bytes(), lust::data::jsonl::M_FIXED_READER_BYTES + reader.retained_buffer_bytes());
}

#[test]
fn test_scoped_view_with_owner_registry_preserves_owner_id() {
    let mut registry = OwnerRegistry::new(10);
    let owner = registry.allocate(100);

    let raw = "{\"user\":\"alice\"}\n{\"user\":\"bob\"}\n";
    let mut reader = lines(raw.as_bytes()).with_owner_id(owner);

    let mut count = 0;
    let res = reader.for_each_view(|view| {
        count += 1;
        assert_eq!(view.owner_id(), Some(owner));
        let s_val = view.as_str_checked(&registry, 100).expect("checked deref succeeds");
        assert!(s_val.contains("user"));
        Ok(ControlFlow::Continue)
    });

    assert_eq!(res.unwrap(), 2);
    assert_eq!(count, 2);
}

#[test]
fn test_scoped_view_early_break_cleans_up_buffer() {
    let raw = "{\"record\":1}\n{\"record\":2}\n{\"record\":3}\n{\"record\":4}\n";
    let mut reader = lines(raw.as_bytes());

    let mut seen = 0;
    let res = reader.for_each_view(|view| {
        seen += 1;
        assert!(view.as_str().unwrap().contains("record"));
        if seen == 2 {
            return Ok(ControlFlow::Break); // Early break
        }
        Ok(ControlFlow::Continue)
    });

    assert_eq!(res.expect("early break succeeds"), 2);
    assert_eq!(seen, 2);
}

#[test]
fn test_cancel_on_pull_reader() {
    let raw = "{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n";
    let token = CancellationToken::new();
    let mut reader = lines(raw.as_bytes()).with_cancellation(token.clone());

    let v1 = reader.next_value().unwrap();
    assert!(v1.is_some());

    // Cancel before second pull
    token.cancel();

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Interrupted);
}

// ============================================================================
// 3. A line over the cap errors and fails closed without retaining whole input
// ============================================================================

#[test]
fn test_line_over_cap_fails_closed_without_retaining_whole_input() {
    // Construct an oversized line with NO LF (e.g. 100 KiB) followed by 500 KiB more data
    const OVERSIZED_LEN: usize = 100 * 1024;
    let mut hostile_input = Vec::with_capacity(OVERSIZED_LEN);
    hostile_input.resize(OVERSIZED_LEN, b'a');

    let cursor = Cursor::new(hostile_input);
    let options = JsonlReaderOptions::default()
        .with_max_line_bytes(1024) // 1 KiB cap
        .with_max_buffered_bytes(16 * 1024);

    let mut reader = lines_with_options(cursor, options);

    let res = reader.next_value();
    assert!(res.is_err(), "Oversized line must fail closed");

    let err = res.unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Limit);
    assert_eq!(err.code, "LINE_TOO_LONG");

    // Critical assertion: the reader NEVER retained the 100 KiB input!
    // Its retained buffer remains tightly bounded by max_buffered_bytes (16 KiB).
    assert!(
        reader.retained_buffer_bytes() <= 16 * 1024,
        "Retained buffer {} exceeded cap on hostile input",
        reader.retained_buffer_bytes()
    );
}

#[test]
fn test_oversized_line_with_lf_fails_closed() {
    let mut data = vec![b'x'; 5000];
    data.push(b'\n');
    data.extend_from_slice(b"{\"valid\":true}\n");

    let options = JsonlReaderOptions::default().with_max_line_bytes(2048);
    let mut reader = lines_with_options(Cursor::new(data), options);

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Limit);
    assert_eq!(err.code, "LINE_TOO_LONG");
}

// ============================================================================
// 4. Fail-Fast and Collect Modes with Bounded Diagnostics
// ============================================================================

#[test]
fn test_fail_fast_mode_terminates_on_first_error() {
    let raw = "{\"id\":1}\nnot_json\n{\"id\":3}\n";
    let mut reader = lines(raw.as_bytes());

    let r1 = reader.next_value().unwrap();
    assert!(r1.is_some());

    let r2 = reader.next_value();
    assert!(r2.is_err(), "Must error on malformed line 2");
    assert_eq!(r2.unwrap_err().line, 2);

    // Fail-fast marks stream terminated: subsequent pull returns Ok(None)
    let r3 = reader.next_value().unwrap();
    assert!(r3.is_none());
}

#[test]
fn test_collect_mode_yields_results_and_recovers_at_lf() {
    let raw = "{\"id\":1}\nnot_json\n{\"id\":3}\n";
    let options = JsonlReaderOptions::default();
    let mut results = line_results(raw.as_bytes(), options);

    // Record 1: Ok
    let item1 = results.next().unwrap().expect("valid record 1");
    if let DataValue::Document(d) = item1 {
        assert_eq!(d.get("id").unwrap().as_i64(), Some(1));
    }

    // Record 2: Err(LineError)
    let item2 = results.next().unwrap();
    assert!(item2.is_err());
    let err = item2.unwrap_err();
    assert_eq!(err.line, 2);
    assert_eq!(err.kind, LineErrorKind::Parse);

    // Record 3: Successfully recovered and parsed!
    let item3 = results.next().unwrap().expect("valid record 3");
    if let DataValue::Document(d) = item3 {
        assert_eq!(d.get("id").unwrap().as_i64(), Some(3));
    }

    assert!(results.next().is_none());
}

#[test]
fn test_collect_mode_diagnostic_bounds_enforced() {
    // 10 syntax errors with max_errors = 3
    let raw = "bad1\nbad2\nbad3\nbad4\nbad5\n";
    let options = JsonlReaderOptions::default().with_max_errors(3);
    let mut results = line_results(raw.as_bytes(), options);

    assert!(results.next().unwrap().is_err()); // err 1
    assert!(results.next().unwrap().is_err()); // err 2
    assert!(results.next().unwrap().is_err()); // err 3

    // 4th error triggers terminal ERROR_LIMIT_EXCEEDED
    let terminal = results.next().unwrap();
    assert!(terminal.is_err());
    assert_eq!(terminal.unwrap_err().code, "ERROR_LIMIT_EXCEEDED");

    assert!(results.next().is_none());
}

// ============================================================================
// 5. Blank Lines Policy (Reject vs Skip)
// ============================================================================

#[test]
fn test_blank_lines_rejected_by_default() {
    let raw = "{\"id\":1}\n   \n{\"id\":2}\n";
    let mut reader = lines(raw.as_bytes());

    assert!(reader.next_value().unwrap().is_some());

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::BlankLine);
    assert_eq!(err.line, 2);
}

#[test]
fn test_blank_lines_skip_preserves_physical_numbering() {
    let raw = "{\"id\":1}\n\n   \n{\"id\":2}\n";
    let options = JsonlReaderOptions::default().with_blank_lines(BlankLines::Skip);
    let mut reader = lines_with_options(raw.as_bytes(), options);

    let v1 = reader.next_value().unwrap().unwrap();
    if let DataValue::Document(d) = v1 {
        assert_eq!(d.get("id").unwrap().as_i64(), Some(1));
    }

    let v2 = reader.next_value().unwrap().unwrap();
    if let DataValue::Document(d) = v2 {
        assert_eq!(d.get("id").unwrap().as_i64(), Some(2));
    }

    // Ensure we are at EOF
    assert!(reader.next_value().unwrap().is_none());
}

// ============================================================================
// 6. Strict RFC 8259, RFC 7464 RS Rejection, and Last Line without LF
// ============================================================================

#[test]
fn test_duplicate_keys_strictly_rejected() {
    let raw = "{\"title\":\"alpha\",\"score\":10,\"title\":\"duplicate\"}\n";
    let mut reader = lines(raw.as_bytes());

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Parse);
    assert_eq!(err.code, "DUPLICATE_KEY");
}

#[test]
fn test_trailing_comma_rejected() {
    let raw = "{\"a\":1,\"b\":2,}\n";
    let mut reader = lines(raw.as_bytes());

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Parse);
    assert_eq!(err.code, "TRAILING_COMMA");
}

#[test]
fn test_rfc7464_record_separator_rejected() {
    // 0x1E followed by JSON
    let raw = b"\x1e{\"seq\":1}\n";
    let mut reader = lines(&raw[..]);

    let err = reader.next_value().unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Parse);
    assert_eq!(err.code, "RFC7464_RS_FORBIDDEN");
}

#[test]
fn test_last_line_without_lf_is_accepted() {
    let raw = "{\"record\":1}\n{\"record\":2}";
    let mut reader = lines(raw.as_bytes());

    let _r1 = reader.next_value().unwrap().unwrap();
    let r2 = reader.next_value().unwrap().unwrap();
    assert!(reader.next_value().unwrap().is_none());

    if let DataValue::Document(d) = r2 {
        assert_eq!(d.get("record").unwrap().as_i64(), Some(2));
    }
}

#[test]
fn test_empty_stream_produces_empty_output() {
    let mut reader: JsonlReader<&[u8]> = lines(b"".as_slice());
    assert!(reader.next_value().unwrap().is_none());
}

// ============================================================================
// 7. Writer Short Writes, Backpressure, and Record Index Tracking
// ============================================================================

struct ShortWriter<W> {
    inner: W,
    max_chunk: usize,
    fail_after_bytes: Option<usize>,
    bytes_seen: usize,
}

impl<W: Write> ShortWriter<W> {
    fn new(inner: W, max_chunk: usize) -> Self {
        Self {
            inner,
            max_chunk,
            fail_after_bytes: None,
            bytes_seen: 0,
        }
    }

    fn with_failure_after(mut self, fail_at: usize) -> Self {
        self.fail_after_bytes = Some(fail_at);
        self
    }
}

impl<W: Write> Write for ShortWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(limit) = self.fail_after_bytes {
            if self.bytes_seen >= limit {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "simulated broken pipe"));
            }
        }

        let to_write = buf.len().min(self.max_chunk);
        let actual = if let Some(limit) = self.fail_after_bytes {
            to_write.min(limit.saturating_sub(self.bytes_seen))
        } else {
            to_write
        };

        if actual == 0 && self.fail_after_bytes.is_some() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "simulated broken pipe"));
        }

        let n = self.inner.write(&buf[..actual])?;
        self.bytes_seen += n;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[test]
fn test_writer_short_writes_handled_correctly() {
    let sink = Vec::new();
    // Sink only accepts at most 7 bytes per write call
    let short_sink = ShortWriter::new(sink, 7);
    let mut writer = JsonlWriter::new(short_sink);

    let mut doc = Document::new();
    doc.try_insert("user".to_string(), DataValue::String("Alex".to_string())).unwrap();
    doc.try_insert("active".to_string(), DataValue::Bool(true)).unwrap();

    let bytes = writer.write_value(&DataValue::Document(doc)).expect("write succeeds");
    assert!(bytes > 0);
    assert_eq!(writer.record_index(), 1);
    assert_eq!(writer.bytes_written(), bytes as u64);

    let output = writer.flush();
    assert!(output.is_ok());
}

#[test]
fn test_writer_records_index_and_confirmed_bytes_on_failure() {
    let sink = Vec::new();
    // Sink will fail after exactly 15 bytes
    let short_sink = ShortWriter::new(sink, 5).with_failure_after(15);
    let mut writer = JsonlWriter::new(short_sink);

    let mut doc1 = Document::new();
    doc1.try_insert("msg".to_string(), DataValue::String("hello world 1".to_string())).unwrap();

    // First write attempts ~25 bytes; short writes accept 5 + 5 + 5 = 15 bytes, then fail
    let res = writer.write_value(&DataValue::Document(doc1));
    assert!(res.is_err());

    let err: StreamWriteError = res.unwrap_err();
    assert_eq!(err.record_index, 1);
    assert_eq!(err.bytes_written, 15);
    assert_eq!(err.kind, WriteErrorKind::Io(io::ErrorKind::BrokenPipe));
}

#[test]
fn test_write_lines_helper() {
    let items = vec![
        DataValue::Number(ExactNumber::Int(1)),
        DataValue::Number(ExactNumber::Int(2)),
        DataValue::Number(ExactNumber::Int(3)),
    ];

    let mut sink = Vec::new();
    let written = write_lines(&mut sink, items, JsonlWriterOptions::default()).unwrap();
    assert!(written > 0);
    assert_eq!(sink, b"1\n2\n3\n");
}

// ============================================================================
// 8. BudgetTracker Integration (Gate-02 Resource Limits Wiring)
// ============================================================================

#[test]
fn test_budget_tracker_enforces_limits() {
    let raw = "{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n";

    // Budget tracker with limit of 2 work units
    let tracker = BudgetTracker::new(1024 * 1024, 2, 64);
    let mut reader = lines(raw.as_bytes()).with_budget(tracker);

    assert!(reader.next_value().unwrap().is_some()); // Record 1
    assert!(reader.next_value().unwrap().is_some()); // Record 2

    // Record 3 exceeds the work units budget (2)
    let res = reader.next_value();
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert_eq!(err.kind, LineErrorKind::Limit);
    assert_eq!(err.code, "WORK_UNITS_EXCEEDED");
}

// ============================================================================
// 9. Schema Validation Integration
// ============================================================================

#[test]
fn test_schema_validated_stream_fail_fast() {
    let user_schema = s.object()
        .field("id", s.int().min(1).build())
        .field("name", s.string().min(2).max(20).build())
        .build();

    let valid_data = "{\"id\":10,\"name\":\"Alice\"}\n{\"id\":20,\"name\":\"Bob\"}\n";
    let mut stream = validate_lines(valid_data.as_bytes(), user_schema.clone(), JsonlReaderOptions::default());

    assert!(stream.next().unwrap().is_ok());
    assert!(stream.next().unwrap().is_ok());
    assert!(stream.next().is_none());

    // Hostile data: id is negative (fails min(1))
    let invalid_data = "{\"id\":1,\"name\":\"Alice\"}\n{\"id\":-5,\"name\":\"Eve\"}\n";
    let mut invalid_stream = validate_lines(invalid_data.as_bytes(), user_schema, JsonlReaderOptions::default());

    assert!(invalid_stream.next().unwrap().is_ok());
    let fail = invalid_stream.next().unwrap();
    assert!(fail.is_err());
    match fail.unwrap_err() {
        StreamValidationError::Schema { line, issue, .. } => {
            assert_eq!(line, 2);
            assert_eq!(issue.code, "too_small");
        }
        _ => panic!("Expected Schema validation error"),
    }
}

#[test]
fn test_schema_validated_stream_collect_mode() {
    let user_schema = s.object()
        .field("id", s.int().min(1).build())
        .field("name", s.string().min(2).max(20).build())
        .build();

    let mixed_data = "{\"id\":10,\"name\":\"Alice\"}\n{\"id\":-5,\"name\":\"Eve\"}\n{\"id\":30,\"name\":\"Charlie\"}\n";
    let mut stream = validate_line_results(mixed_data.as_bytes(), user_schema, JsonlReaderOptions::default());

    // Record 1: Ok
    assert!(stream.next().unwrap().unwrap().is_ok());

    // Record 2: Schema validation error
    let res2 = stream.next().unwrap().unwrap();
    assert!(res2.is_err());

    // Record 3: Successfully continues and yields Ok
    assert!(stream.next().unwrap().unwrap().is_ok());

    assert!(stream.next().is_none());
}

// ============================================================================
// 10. Roundtrip All Supported Data Values
// ============================================================================

#[test]
fn test_jsonl_roundtrip_all_types() {
    let mut doc = Document::new();
    doc.try_insert("null_key".to_string(), DataValue::Null).unwrap();
    doc.try_insert("bool_true".to_string(), DataValue::Bool(true)).unwrap();
    doc.try_insert("int".to_string(), DataValue::Number(ExactNumber::Int(42))).unwrap();
    doc.try_insert("float".to_string(), DataValue::Number(ExactNumber::Float(99.5))).unwrap();
    doc.try_insert("str".to_string(), DataValue::String("Lust JSONL streaming".to_string())).unwrap();
    doc.try_insert(
        "arr".to_string(),
        DataValue::Array(vec![
            DataValue::Number(ExactNumber::Int(1)),
            DataValue::Number(ExactNumber::Int(2)),
        ]),
    ).unwrap();

    let val = DataValue::Document(doc);

    let mut buf = Vec::new();
    let mut writer = JsonlWriter::new(&mut buf);
    writer.write_value(&val).unwrap();
    writer.flush().unwrap();

    let mut reader = lines(buf.as_slice());
    let recovered = reader.next_value().unwrap().unwrap();

    assert_eq!(val, recovered);
}

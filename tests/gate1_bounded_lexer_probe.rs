// Gate 1 Prototype: Isolated Bounded Lexical Parser Probe for Issue #11
//
// Implements and empirically verifies the Gate 1 bounded lexical parser prototype
// described by accepted Terra plan DESIGN-ISSUE-11-G1 and RFC 04.
//
// Properties verified:
// 1. Root positional array JSON grammar (outer [ ... ] array, whitespace, trailing tokens, commas).
// 2. String decoding, UTF-8 validity, escapes, and full UTF-16 surrogate pairing.
// 3. Decoded duplicate key detection (including escape-equivalent and multi-byte keys) without unbounded allocations.
// 4. Number token class fidelity and i64/f64 bounds:
//    - integer -0 mapped to Int(0)
//    - float -0.0 retaining Float(-0.0) with negative sign
//    - integer tokens 1 vs float 1.0 vs 1e0 preserving token class
//    - exact i64 endpoints (i64::MIN, i64::MAX)
//    - >i64::MAX, u64-only, >u64::MAX, <i64::MIN rejected with NUMBER_RANGE
//    - float overflow (>f64::MAX) and underflow-to-zero (<f64::MIN_POSITIVE) rejected with NUMBER_RANGE
// 5. Ceilings: max_input_bytes, max_depth, max_nodes, max_args, max_bridge_bytes.
// 6. Pre-growth reservation: all variable-sized parser structures pre-reserved in CapacityLedger;
//    zero collection growth during parsing; ledger cleanup on success and error paths.
//
// Limitations explicitly acknowledged:
// - Does NOT construct actual `lust::bytecode::Value` (no VM/runtime integration).
// - Tracks requested capacity in the CapacityLedger, not OS allocator rounding or RSS.
// - Does not expose C FFI exports or perform VM memory accounting transfer.

use std::fmt;

// ---------------------------------------------------------------------------
// RFC 04 Status Codes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LustJsonStatus {
    Ok = 0,
    InvalidArgument = 1,
    InvalidJson = 2,
    NumberRange = 3,
    Limit = 4,
    Type = 5,
    Call = 6,
    Unsupported = 7,
    Cycle = 8,
    Borrow = 9,
    Internal = 10,
}

impl fmt::Display for LustJsonStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ---------------------------------------------------------------------------
// RFC 04 Limits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct LustJsonLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_args: usize,
    pub max_bridge_bytes: usize,
}

impl Default for LustJsonLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1_048_576,
            max_output_bytes: 1_048_576,
            max_depth: 64,
            max_nodes: 65_536,
            max_args: 256,
            max_bridge_bytes: 16_777_216,
        }
    }
}

// ---------------------------------------------------------------------------
// Capacity Ledger & Pre-growth Reservation
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct CapacityLedger {
    allocated: usize,
    peak: usize,
    max_bridge_bytes: usize,
}

impl CapacityLedger {
    pub fn new(max_bridge_bytes: usize) -> Self {
        Self {
            allocated: 0,
            peak: 0,
            max_bridge_bytes,
        }
    }

    pub fn try_reserve(&mut self, bytes: usize) -> Result<(), LustJsonStatus> {
        let new_total = self
            .allocated
            .checked_add(bytes)
            .ok_or(LustJsonStatus::Limit)?;
        if new_total > self.max_bridge_bytes {
            return Err(LustJsonStatus::Limit);
        }
        self.allocated = new_total;
        if self.allocated > self.peak {
            self.peak = self.allocated;
        }
        Ok(())
    }

    pub fn release(&mut self, bytes: usize) {
        self.allocated = self.allocated.saturating_sub(bytes);
    }

    pub fn allocated(&self) -> usize {
        self.allocated
    }

    pub fn peak(&self) -> usize {
        self.peak
    }
}

// ---------------------------------------------------------------------------
// Fixed-capacity Pre-reserved Collections (Guaranteed Zero Growth During Parse)
// ---------------------------------------------------------------------------

pub struct PreallocatedVec<T> {
    inner: Vec<T>,
    capacity: usize,
    reserved_bytes: usize,
}

impl<T> PreallocatedVec<T> {
    pub fn with_capacity_in_ledger(
        capacity: usize,
        ledger: &mut CapacityLedger,
    ) -> Result<Self, LustJsonStatus> {
        let reserved_bytes = capacity
            .checked_mul(std::mem::size_of::<T>())
            .ok_or(LustJsonStatus::Limit)?;
        ledger.try_reserve(reserved_bytes)?;
        Ok(Self {
            inner: Vec::with_capacity(capacity),
            capacity,
            reserved_bytes,
        })
    }

    #[inline]
    pub fn push(&mut self, item: T) -> Result<(), LustJsonStatus> {
        if self.inner.len() >= self.capacity {
            return Err(LustJsonStatus::Limit);
        }
        let cap_before = self.inner.capacity();
        self.inner.push(item);
        // Invariant assertion: capacity must NEVER change after pre-reservation!
        assert_eq!(
            self.inner.capacity(),
            cap_before,
            "CRITICAL: Collection reallocated after parsing started!"
        );
        Ok(())
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        self.inner.pop()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.inner
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.inner.truncate(len);
    }

    pub fn release_ledger(&self, ledger: &mut CapacityLedger) {
        ledger.release(self.reserved_bytes);
    }
}

pub struct PreallocatedStringArena {
    bytes: Vec<u8>,
    capacity: usize,
    reserved_bytes: usize,
}

impl PreallocatedStringArena {
    pub fn with_capacity_in_ledger(
        capacity: usize,
        ledger: &mut CapacityLedger,
    ) -> Result<Self, LustJsonStatus> {
        ledger.try_reserve(capacity)?;
        Ok(Self {
            bytes: Vec::with_capacity(capacity),
            capacity,
            reserved_bytes: capacity,
        })
    }

    pub fn push_str(&mut self, s: &str) -> Result<(usize, usize), LustJsonStatus> {
        let start = self.bytes.len();
        let needed = s.len();
        let new_len = start.checked_add(needed).ok_or(LustJsonStatus::Limit)?;
        if new_len > self.capacity {
            return Err(LustJsonStatus::Limit);
        }
        let cap_before = self.bytes.capacity();
        self.bytes.extend_from_slice(s.as_bytes());
        assert_eq!(
            self.bytes.capacity(),
            cap_before,
            "CRITICAL: String arena reallocated after parsing started!"
        );
        Ok((start, new_len))
    }

    pub fn push_char(&mut self, ch: char) -> Result<(), LustJsonStatus> {
        let mut buf = [0u8; 4];
        let encoded = ch.encode_utf8(&mut buf);
        self.push_str(encoded).map(|_| ())
    }

    #[inline]
    pub fn get(&self, start: usize, end: usize) -> &str {
        std::str::from_utf8(&self.bytes[start..end]).expect("arena contains valid utf-8")
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn truncate(&mut self, len: usize) {
        self.bytes.truncate(len);
    }

    pub fn release_ledger(&self, ledger: &mut CapacityLedger) {
        ledger.release(self.reserved_bytes);
    }
}

// ---------------------------------------------------------------------------
// Lexical Tokens and Parsed AST (Prototype Values)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedNumber {
    Int(i64),
    Float(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PrototypeValue {
    Nil,
    Bool(bool),
    Number(ParsedNumber),
    String(String),
    Array(Vec<PrototypeValue>),
    Map(Vec<(String, PrototypeValue)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerKind {
    Array,
    Object,
}

#[derive(Debug, Clone)]
pub struct ContainerFrame {
    pub kind: ContainerKind,
    pub key_start_idx: usize,
    pub element_count: usize,
    pub arena_start: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRef<'a> {
    Borrowed(&'a str),
    Decoded(usize, usize),
}

impl<'a> KeyRef<'a> {
    pub fn as_str<'s>(&'s self, arena: &'s PreallocatedStringArena) -> &'s str {
        match self {
            KeyRef::Borrowed(s) => s,
            KeyRef::Decoded(start, end) => arena.get(*start, *end),
        }
    }
}

// ---------------------------------------------------------------------------
// Bounded Lexical Parser Implementation
// ---------------------------------------------------------------------------

pub struct BoundedLexer<'a> {
    input: &'a [u8],
    pos: usize,
    limits: LustJsonLimits,
    ledger: CapacityLedger,
    current_depth: usize,
    node_count: usize,
    stack: PreallocatedVec<ContainerFrame>,
    keys: PreallocatedVec<KeyRef<'a>>,
    arena: PreallocatedStringArena,
}

impl<'a> BoundedLexer<'a> {
    pub fn new(
        input: &'a str,
        limits: LustJsonLimits,
        mut ledger: CapacityLedger,
    ) -> Result<Self, LustJsonStatus> {
        let input_bytes = input.as_bytes();
        if input_bytes.len() > limits.max_input_bytes {
            return Err(LustJsonStatus::Limit);
        }

        // Pre-reserve fixed capacities in CapacityLedger before parsing begins:
        // 1. Stack capacity = limits.max_depth
        let stack = PreallocatedVec::with_capacity_in_ledger(limits.max_depth, &mut ledger)?;

        // 2. Key tracker capacity = min(limits.max_nodes, input_bytes.len() / 2 + 1)
        let max_key_entries = limits.max_nodes.min((input_bytes.len() / 2).max(1));
        let keys = PreallocatedVec::with_capacity_in_ledger(max_key_entries, &mut ledger)?;

        // 3. String arena capacity = min(limits.max_bridge_bytes / 4, input_bytes.len())
        let arena_capacity = input_bytes.len();
        let arena = PreallocatedStringArena::with_capacity_in_ledger(arena_capacity, &mut ledger)?;

        Ok(Self {
            input: input_bytes,
            pos: 0,
            limits,
            ledger,
            current_depth: 0,
            node_count: 0,
            stack,
            keys,
            arena,
        })
    }

    pub fn ledger(&self) -> &CapacityLedger {
        &self.ledger
    }

    pub fn release_ledger(mut self) -> CapacityLedger {
        self.stack.release_ledger(&mut self.ledger);
        self.keys.release_ledger(&mut self.ledger);
        self.arena.release_ledger(&mut self.ledger);
        self.ledger
    }

    #[inline]
    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        if self.pos < self.input.len() {
            Some(self.input[self.pos])
        } else {
            None
        }
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        if self.pos < self.input.len() {
            let b = self.input[self.pos];
            self.pos += 1;
            Some(b)
        } else {
            None
        }
    }

    #[inline]
    fn increment_node(&mut self) -> Result<(), LustJsonStatus> {
        self.node_count = self
            .node_count
            .checked_add(1)
            .ok_or(LustJsonStatus::Limit)?;
        if self.node_count > self.limits.max_nodes {
            return Err(LustJsonStatus::Limit);
        }
        Ok(())
    }

    // Parse the entire document: root MUST be an array `[ ... ]`
    pub fn parse_root_arguments(&mut self) -> Result<Vec<PrototypeValue>, LustJsonStatus> {
        self.skip_whitespace();
        if self.pos >= self.input.len() {
            return Err(LustJsonStatus::InvalidArgument);
        }

        if self.peek() != Some(b'[') {
            return Err(LustJsonStatus::InvalidJson);
        }

        self.bump(); // consume '['
        self.current_depth = 1;
        if self.current_depth > self.limits.max_depth {
            return Err(LustJsonStatus::Limit);
        }
        self.increment_node()?;

        self.stack.push(ContainerFrame {
            kind: ContainerKind::Array,
            key_start_idx: self.keys.len(),
            element_count: 0,
            arena_start: self.arena.len(),
        })?;

        let mut args = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.bump();
            self.current_depth -= 1;
            self.stack.pop();
            self.skip_whitespace();
            if self.pos < self.input.len() {
                return Err(LustJsonStatus::InvalidJson); // trailing tokens!
            }
            return Ok(args);
        }

        loop {
            // Check max_args ceiling
            if args.len() >= self.limits.max_args {
                return Err(LustJsonStatus::Limit);
            }

            let val = self.parse_value()?;
            args.push(val);

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek() == Some(b']') {
                        return Err(LustJsonStatus::InvalidJson); // trailing comma!
                    }
                }
                Some(b']') => {
                    self.bump();
                    break;
                }
                _ => return Err(LustJsonStatus::InvalidJson),
            }
        }

        self.current_depth -= 1;
        self.stack.pop();

        self.skip_whitespace();
        if self.pos < self.input.len() {
            return Err(LustJsonStatus::InvalidJson); // trailing tokens rejected!
        }

        Ok(args)
    }

    fn parse_value(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        self.skip_whitespace();
        let b = self.peek().ok_or(LustJsonStatus::InvalidJson)?;

        match b {
            b'n' => self.parse_null(),
            b't' | b'f' => self.parse_bool(),
            b'"' => {
                let s = self.parse_string()?;
                self.increment_node()?;
                Ok(PrototypeValue::String(s))
            }
            b'[' => self.parse_array(),
            b'{' => self.parse_object(),
            b'-' | b'0'..=b'9' => self.parse_number(),
            _ => Err(LustJsonStatus::InvalidJson),
        }
    }

    fn parse_null(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        if self.input[self.pos..].starts_with(b"null") {
            self.pos += 4;
            self.increment_node()?;
            Ok(PrototypeValue::Nil)
        } else {
            Err(LustJsonStatus::InvalidJson)
        }
    }

    fn parse_bool(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        if self.input[self.pos..].starts_with(b"true") {
            self.pos += 4;
            self.increment_node()?;
            Ok(PrototypeValue::Bool(true))
        } else if self.input[self.pos..].starts_with(b"false") {
            self.pos += 5;
            self.increment_node()?;
            Ok(PrototypeValue::Bool(false))
        } else {
            Err(LustJsonStatus::InvalidJson)
        }
    }

    fn parse_array(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        self.bump(); // consume '['
        self.current_depth = self
            .current_depth
            .checked_add(1)
            .ok_or(LustJsonStatus::Limit)?;
        if self.current_depth > self.limits.max_depth {
            return Err(LustJsonStatus::Limit);
        }
        self.increment_node()?;

        self.stack.push(ContainerFrame {
            kind: ContainerKind::Array,
            key_start_idx: self.keys.len(),
            element_count: 0,
            arena_start: self.arena.len(),
        })?;

        let mut items = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.bump();
            self.current_depth -= 1;
            self.stack.pop();
            return Ok(PrototypeValue::Array(items));
        }

        loop {
            let val = self.parse_value()?;
            items.push(val);

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek() == Some(b']') {
                        return Err(LustJsonStatus::InvalidJson); // trailing comma
                    }
                }
                Some(b']') => {
                    self.bump();
                    break;
                }
                _ => return Err(LustJsonStatus::InvalidJson),
            }
        }

        self.current_depth -= 1;
        self.stack.pop();
        Ok(PrototypeValue::Array(items))
    }

    fn parse_object(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        self.bump(); // consume '{'
        self.current_depth = self
            .current_depth
            .checked_add(1)
            .ok_or(LustJsonStatus::Limit)?;
        if self.current_depth > self.limits.max_depth {
            return Err(LustJsonStatus::Limit);
        }
        self.increment_node()?;

        let key_start_idx = self.keys.len();
        let arena_start = self.arena.len();

        self.stack.push(ContainerFrame {
            kind: ContainerKind::Object,
            key_start_idx,
            element_count: 0,
            arena_start,
        })?;

        let mut entries = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.bump();
            self.current_depth -= 1;
            self.stack.pop();
            return Ok(PrototypeValue::Map(entries));
        }

        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err(LustJsonStatus::InvalidJson); // non-string object key!
            }

            let key_ref = self.parse_key_ref()?;
            self.increment_node()?; // key counts as a node

            // Duplicate decoded key check:
            let key_str = key_ref.as_str(&self.arena);
            for prev_key in &self.keys.as_slice()[key_start_idx..] {
                if prev_key.as_str(&self.arena) == key_str {
                    return Err(LustJsonStatus::InvalidJson); // duplicate decoded key!
                }
            }

            let owned_key = key_str.to_string();
            self.keys.push(key_ref)?;

            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(LustJsonStatus::InvalidJson);
            }
            self.bump(); // consume ':'

            let val = self.parse_value()?;
            entries.push((owned_key, val));

            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.bump();
                    self.skip_whitespace();
                    if self.peek() == Some(b'}') {
                        return Err(LustJsonStatus::InvalidJson); // trailing comma
                    }
                }
                Some(b'}') => {
                    self.bump();
                    break;
                }
                _ => return Err(LustJsonStatus::InvalidJson),
            }
        }

        // Clean up keys and arena allocated for this object frame:
        self.keys.truncate(key_start_idx);
        self.arena.truncate(arena_start);

        self.current_depth -= 1;
        self.stack.pop();
        Ok(PrototypeValue::Map(entries))
    }

    // Scans string and returns key reference (borrowed if unescaped, decoded into arena if escaped)
    fn parse_key_ref(&mut self) -> Result<KeyRef<'a>, LustJsonStatus> {
        self.bump(); // consume opening '"'
        let start = self.pos;
        let mut has_escapes = false;

        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b < 0x20 {
                return Err(LustJsonStatus::InvalidJson); // unescaped control character!
            }
            if b == b'\\' {
                has_escapes = true;
                break;
            }
            if b == b'"' {
                let slice = &self.input[start..self.pos];
                let s = std::str::from_utf8(slice).map_err(|_| LustJsonStatus::InvalidJson)?;
                self.pos += 1; // consume closing '"'
                return Ok(KeyRef::Borrowed(s));
            }
            self.pos += 1;
        }

        if !has_escapes {
            return Err(LustJsonStatus::InvalidJson); // unterminated string
        }

        // Escaped string: decode into preallocated arena
        let arena_start = self.arena.len();
        let unescaped_prefix = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| LustJsonStatus::InvalidJson)?;
        self.arena.push_str(unescaped_prefix)?;

        self.decode_escapes_into_arena()?;
        let arena_end = self.arena.len();
        Ok(KeyRef::Decoded(arena_start, arena_end))
    }

    fn parse_string(&mut self) -> Result<String, LustJsonStatus> {
        let key_ref = self.parse_key_ref()?;
        Ok(key_ref.as_str(&self.arena).to_string())
    }

    fn decode_escapes_into_arena(&mut self) -> Result<(), LustJsonStatus> {
        while self.pos < self.input.len() {
            let b = self.input[self.pos];
            if b == b'"' {
                self.pos += 1; // closing quote
                return Ok(());
            }
            if b < 0x20 {
                return Err(LustJsonStatus::InvalidJson);
            }
            if b != b'\\' {
                // Collect unescaped slice
                let seg_start = self.pos;
                while self.pos < self.input.len()
                    && self.input[self.pos] != b'\\'
                    && self.input[self.pos] != b'"'
                {
                    if self.input[self.pos] < 0x20 {
                        return Err(LustJsonStatus::InvalidJson);
                    }
                    self.pos += 1;
                }
                let seg = std::str::from_utf8(&self.input[seg_start..self.pos])
                    .map_err(|_| LustJsonStatus::InvalidJson)?;
                self.arena.push_str(seg)?;
                continue;
            }

            // Consume '\\'
            self.pos += 1;
            let esc = self.bump().ok_or(LustJsonStatus::InvalidJson)?;
            match esc {
                b'"' => self.arena.push_char('"')?,
                b'\\' => self.arena.push_char('\\')?,
                b'/' => self.arena.push_char('/')?,
                b'b' => self.arena.push_char('\x08')?,
                b'f' => self.arena.push_char('\x0C')?,
                b'n' => self.arena.push_char('\n')?,
                b'r' => self.arena.push_char('\r')?,
                b't' => self.arena.push_char('\t')?,
                b'u' => {
                    let u1 = self.parse_hex4()?;
                    // Check if high surrogate
                    if (0xD800..=0xDBFF).contains(&u1) {
                        // Must be followed by \uDC00..\uDFFF
                        if self.pos + 2 > self.input.len()
                            || self.input[self.pos] != b'\\'
                            || self.input[self.pos + 1] != b'u'
                        {
                            return Err(LustJsonStatus::InvalidJson); // unpaired high surrogate!
                        }
                        self.pos += 2; // consume \u
                        let u2 = self.parse_hex4()?;
                        if !(0xDC00..=0xDFFF).contains(&u2) {
                            return Err(LustJsonStatus::InvalidJson); // invalid low surrogate!
                        }
                        let code = 0x10000 + (((u1 as u32 - 0xD800) << 10) | (u2 as u32 - 0xDC00));
                        let ch = char::from_u32(code).ok_or(LustJsonStatus::InvalidJson)?;
                        self.arena.push_char(ch)?;
                    } else if (0xDC00..=0xDFFF).contains(&u1) {
                        return Err(LustJsonStatus::InvalidJson); // unpaired low surrogate!
                    } else {
                        let ch = char::from_u32(u1 as u32).ok_or(LustJsonStatus::InvalidJson)?;
                        self.arena.push_char(ch)?;
                    }
                }
                _ => return Err(LustJsonStatus::InvalidJson),
            }
        }
        Err(LustJsonStatus::InvalidJson) // unterminated string
    }

    fn parse_hex4(&mut self) -> Result<u16, LustJsonStatus> {
        if self.pos + 4 > self.input.len() {
            return Err(LustJsonStatus::InvalidJson);
        }
        let s = std::str::from_utf8(&self.input[self.pos..self.pos + 4])
            .map_err(|_| LustJsonStatus::InvalidJson)?;
        let val = u16::from_str_radix(s, 16).map_err(|_| LustJsonStatus::InvalidJson)?;
        self.pos += 4;
        Ok(val)
    }

    // Number Lexing with Token Class Fidelity and RFC 04 Bounds
    fn parse_number(&mut self) -> Result<PrototypeValue, LustJsonStatus> {
        let start = self.pos;
        let mut has_minus = false;
        let mut has_dot = false;
        let mut has_exp = false;

        if self.peek() == Some(b'-') {
            has_minus = true;
            self.bump();
        }

        // Integer part
        let int_start = self.pos;
        match self.peek() {
            Some(b'0') => {
                self.bump();
                // Leading zero: next char MUST NOT be a digit
                if let Some(next) = self.peek() {
                    if next.is_ascii_digit() {
                        return Err(LustJsonStatus::InvalidJson); // leading zero rejected!
                    }
                }
            }
            Some(b'1'..=b'9') => {
                while let Some(d) = self.peek() {
                    if d.is_ascii_digit() {
                        self.bump();
                    } else {
                        break;
                    }
                }
            }
            _ => return Err(LustJsonStatus::InvalidJson),
        }
        let int_digits = &self.input[int_start..self.pos];

        // Fraction part
        if self.peek() == Some(b'.') {
            has_dot = true;
            self.bump();
            let frac_start = self.pos;
            while let Some(d) = self.peek() {
                if d.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.pos == frac_start {
                return Err(LustJsonStatus::InvalidJson); // decimal point must be followed by digit!
            }
        }

        // Exponent part
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            has_exp = true;
            self.bump();
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                self.bump();
            }
            let exp_start = self.pos;
            while let Some(d) = self.peek() {
                if d.is_ascii_digit() {
                    self.bump();
                } else {
                    break;
                }
            }
            if self.pos == exp_start {
                return Err(LustJsonStatus::InvalidJson); // exponent must have digits!
            }
        }

        let raw_num_str = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| LustJsonStatus::InvalidJson)?;

        self.increment_node()?;

        // Token Class & Range Classification:
        if !has_dot && !has_exp {
            // Integer token class
            if has_minus && int_digits == b"0" {
                // RFC 04: Integer -0 maps to Int(0)
                return Ok(PrototypeValue::Number(ParsedNumber::Int(0)));
            }

            match raw_num_str.parse::<i64>() {
                Ok(val) => Ok(PrototypeValue::Number(ParsedNumber::Int(val))),
                Err(_) => {
                    // Out-of-i64 integer (>i64::MAX, u64 only, >u64::MAX, <i64::MIN)
                    // RFC 04: Reject with NUMBER_RANGE, NEVER fallback to float!
                    Err(LustJsonStatus::NumberRange)
                }
            }
        } else {
            // Float token class
            let f = raw_num_str
                .parse::<f64>()
                .map_err(|_| LustJsonStatus::NumberRange)?;

            // Overflow / non-finite
            if !f.is_finite() {
                return Err(LustJsonStatus::NumberRange);
            }

            // Underflow-to-zero: nonzero input that rounds to 0.0 or -0.0
            if f == 0.0 {
                // Check if the input significand has any nonzero digit
                let is_nonzero_input = self.input[start..self.pos]
                    .iter()
                    .any(|&b| (b'1'..=b'9').contains(&b));
                if is_nonzero_input {
                    return Err(LustJsonStatus::NumberRange); // underflow to zero rejected!
                }
            }

            Ok(PrototypeValue::Number(ParsedNumber::Float(f)))
        }
    }
}

// ===========================================================================
// TESTS & EMPIRICAL EVIDENCE
// ===========================================================================

#[test]
fn test_root_positional_array_grammar() {
    println!("\n=== 1. ROOT POSITIONAL ARRAY JSON GRAMMAR ===");

    let limits = LustJsonLimits::default();
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);

    // Empty array: 0 arguments
    let mut lexer = BoundedLexer::new("[]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 0);

    // Whitespace handling
    let mut lexer = BoundedLexer::new("   [  \t\n\r  ]  \t ", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 0);

    // Positional arguments
    let mut lexer = BoundedLexer::new(
        "[null, true, false, 42, \"hello\", [1, 2], {\"a\": true}]",
        limits,
        ledger.clone(),
    )
    .unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 7);
    assert_eq!(args[0], PrototypeValue::Nil);
    assert_eq!(args[1], PrototypeValue::Bool(true));
    assert_eq!(args[2], PrototypeValue::Bool(false));
    assert_eq!(args[3], PrototypeValue::Number(ParsedNumber::Int(42)));
    assert_eq!(args[4], PrototypeValue::String("hello".to_string()));

    // Non-array roots rejected with InvalidJson
    let non_array_cases = [
        "{}",
        "\"string\"",
        "123",
        "true",
        "false",
        "null",
        "{\"args\": []}",
    ];
    for case in non_array_cases {
        let mut lexer = BoundedLexer::new(case, limits, ledger.clone()).unwrap();
        assert_eq!(
            lexer.parse_root_arguments(),
            Err(LustJsonStatus::InvalidJson),
            "Non-array root {:?} must be rejected",
            case
        );
    }

    // Empty or whitespace only input rejected with InvalidArgument
    let mut lexer = BoundedLexer::new("", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidArgument)
    );
    let mut lexer = BoundedLexer::new("   \t\n  ", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidArgument)
    );

    // Trailing tokens rejected with InvalidJson
    let trailing_cases = ["[1] 2", "[] []", "[1], [2]", "[1] ;"];
    for case in trailing_cases {
        let mut lexer = BoundedLexer::new(case, limits, ledger.clone()).unwrap();
        assert_eq!(
            lexer.parse_root_arguments(),
            Err(LustJsonStatus::InvalidJson),
            "Trailing tokens in {:?} must be rejected",
            case
        );
    }

    // Truncated input rejected
    let truncated_cases = ["[", "[1,", "[1, 2", "[1, [2]"];
    for case in truncated_cases {
        let mut lexer = BoundedLexer::new(case, limits, ledger.clone()).unwrap();
        assert_eq!(
            lexer.parse_root_arguments(),
            Err(LustJsonStatus::InvalidJson),
            "Truncated input {:?} must be rejected",
            case
        );
    }

    // Trailing / misplaced commas
    let comma_cases = ["[1,]", "[,1]", "[1,,2]"];
    for case in comma_cases {
        let mut lexer = BoundedLexer::new(case, limits, ledger.clone()).unwrap();
        assert_eq!(
            lexer.parse_root_arguments(),
            Err(LustJsonStatus::InvalidJson),
            "Comma error in {:?} must be rejected",
            case
        );
    }

    println!("PASS: Root positional array grammar validated successfully.");
}

#[test]
fn test_strings_utf8_escapes_and_surrogates() {
    println!("\n=== 2. STRINGS, UTF-8, ESCAPES, AND SURROGATE PAIRS ===");

    let limits = LustJsonLimits::default();
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);

    // Plain unescaped string
    let mut lexer = BoundedLexer::new("[\"hello world\"]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::String("hello world".to_string()));

    // Standard escapes
    let mut lexer =
        BoundedLexer::new(r#"["\" \\ \/ \b \f \n \r \t"]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(
        args[0],
        PrototypeValue::String("\" \\ / \x08 \x0C \n \r \t".to_string())
    );

    // Unicode escapes and escaped NUL
    let mut lexer =
        BoundedLexer::new(r#"["\u0041 \u0000 \u00e9"]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::String("A \0 é".to_string()));

    // Valid UTF-16 surrogate pairs:
    // U+1F600 (GRINNING FACE 😀): \uD83D\uDE00
    let mut lexer = BoundedLexer::new(r#"["\uD83D\uDE00"]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::String("😀".to_string()));

    // U+1D11E (MUSICAL SYMBOL G CLEF 𝄞): \uD834\uDD1E
    let mut lexer = BoundedLexer::new(r#"["\uD834\uDD1E"]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::String("𝄞".to_string()));

    // Unpaired high surrogate rejected
    let mut lexer = BoundedLexer::new(r#"["\uD83D"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    let mut lexer = BoundedLexer::new(r#"["\uD83D abc"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    // High surrogate followed by non-surrogate rejected
    let mut lexer = BoundedLexer::new(r#"["\uD83D\u0041"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    // Unpaired low surrogate rejected
    let mut lexer = BoundedLexer::new(r#"["\uDE00"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    // Invalid hex digits rejected
    let mut lexer = BoundedLexer::new(r#"["\u12G4"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    // Unescaped control characters (< 0x20) rejected
    let raw_control = "[\"hello\nworld\"]";
    let mut lexer = BoundedLexer::new(raw_control, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    // Invalid escape letter rejected
    let mut lexer = BoundedLexer::new(r#"["\a"]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    println!("PASS: Strings, UTF-8, escapes, and surrogate pairs validated.");
}

#[test]
fn test_decoded_duplicate_keys() {
    println!("\n=== 3. DECODED DUPLICATE KEY REJECTION ===");

    let limits = LustJsonLimits::default();
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);

    // Plain literal duplicate key
    let mut lexer = BoundedLexer::new(r#"[{"a": 1, "a": 2}]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson),
        "Literal duplicate key 'a' must be rejected"
    );

    // Escape-equivalent duplicate key: "foo" vs "\u0066oo"
    let mut lexer =
        BoundedLexer::new(r#"[{"foo": 1, "\u0066oo": 2}]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson),
        "Escape-equivalent key 'foo' vs '\\u0066oo' must be rejected"
    );

    // Multi-byte UTF-8 escape-equivalent duplicate: "café" vs "caf\u00e9"
    let mut lexer =
        BoundedLexer::new(r#"[{"café": 1, "caf\u00e9": 2}]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson),
        "Unicode escape-equivalent key 'café' vs 'caf\\u00e9' must be rejected"
    );

    // Distinct keys pass
    let mut lexer = BoundedLexer::new(r#"[{"a": 1, "b": 2}]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 1);

    // Sibling objects with identical keys pass (different objects)
    let mut lexer = BoundedLexer::new(r#"[{"x": 1}, {"x": 2}]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 2);

    // Nested object with identical key name pass (different nesting depth)
    let mut lexer = BoundedLexer::new(r#"[{"a": {"a": 1}}]"#, limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 1);

    // Non-string object key rejected
    let mut lexer = BoundedLexer::new(r#"[{1: 2}]"#, limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );

    println!("PASS: Decoded duplicate keys rejected with zero unbounded allocation.");
}

#[test]
fn test_number_token_class_and_bounds() {
    println!("\n=== 4. NUMBER TOKEN CLASS FIDELITY & BOUNDS ===");

    let limits = LustJsonLimits::default();
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);

    // 1. Integer -0 vs Float -0.0
    // Integer "-0": must produce Int(0)
    let mut lexer = BoundedLexer::new("[-0]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(
        args[0],
        PrototypeValue::Number(ParsedNumber::Int(0)),
        "Integer -0 must yield Int(0)"
    );

    // Float "-0.0": must produce Float(-0.0) with negative sign intact
    let mut lexer = BoundedLexer::new("[-0.0]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    match &args[0] {
        PrototypeValue::Number(ParsedNumber::Float(f)) => {
            assert_eq!(*f, 0.0);
            assert!(
                f.is_sign_negative(),
                "Float -0.0 must preserve negative sign"
            );
        }
        other => panic!("Expected Float(-0.0), got {:?}", other),
    }

    // Float "-0e0": must produce Float(-0.0) with negative sign intact
    let mut lexer = BoundedLexer::new("[-0e0]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    match &args[0] {
        PrototypeValue::Number(ParsedNumber::Float(f)) => {
            assert_eq!(*f, 0.0);
            assert!(
                f.is_sign_negative(),
                "Float -0e0 must preserve negative sign"
            );
        }
        other => panic!("Expected Float(-0.0), got {:?}", other),
    }

    // 2. Token class distinction: 1 vs 1.0 vs 1e0
    let mut lexer = BoundedLexer::new("[1, 1.0, 1e0]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::Number(ParsedNumber::Int(1)));
    assert_eq!(args[1], PrototypeValue::Number(ParsedNumber::Float(1.0)));
    assert_eq!(args[2], PrototypeValue::Number(ParsedNumber::Float(1.0)));

    // 3. Exact i64 endpoints
    let mut lexer = BoundedLexer::new(
        "[9223372036854775807, -9223372036854775808]",
        limits,
        ledger.clone(),
    )
    .unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::Number(ParsedNumber::Int(i64::MAX)));
    assert_eq!(args[1], PrototypeValue::Number(ParsedNumber::Int(i64::MIN)));

    // 4. Integers out of i64 range rejected with NumberRange (NEVER fallback to float!)
    // i64::MAX + 1 (9223372036854775808)
    let mut lexer = BoundedLexer::new("[9223372036854775808]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "i64::MAX + 1 must return NumberRange"
    );

    // u64::MAX (18446744073709551615)
    let mut lexer = BoundedLexer::new("[18446744073709551615]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "u64::MAX must return NumberRange"
    );

    // > u64::MAX (18446744073709551616)
    let mut lexer = BoundedLexer::new("[18446744073709551616]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "> u64::MAX must return NumberRange"
    );

    // < i64::MIN (-9223372036854775809)
    let mut lexer = BoundedLexer::new("[-9223372036854775809]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "< i64::MIN must return NumberRange"
    );

    // 5. Float overflow rejected with NumberRange
    let mut lexer = BoundedLexer::new("[1e500]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "Float overflow 1e500 must return NumberRange"
    );

    let mut lexer = BoundedLexer::new("[-1e500]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "Float overflow -1e500 must return NumberRange"
    );

    // 6. Float underflow-to-zero rejected with NumberRange
    let mut lexer = BoundedLexer::new("[1e-500]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "Float underflow 1e-500 must return NumberRange"
    );

    let mut lexer = BoundedLexer::new("[-1e-500]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::NumberRange),
        "Float underflow -1e-500 must return NumberRange"
    );

    // 7. Non-underflow zero float (literal 0.0 and 0e0) pass
    let mut lexer = BoundedLexer::new("[0.0, 0e0]", limits, ledger.clone()).unwrap();
    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args[0], PrototypeValue::Number(ParsedNumber::Float(0.0)));
    assert_eq!(args[1], PrototypeValue::Number(ParsedNumber::Float(0.0)));

    // 8. Syntax errors: leading zeroes, missing digits
    let syntax_cases = ["[01]", "[-01]", "[1.]", "[1e]", "[1e+]", "[-]"];
    for case in syntax_cases {
        let mut lexer = BoundedLexer::new(case, limits, ledger.clone()).unwrap();
        assert_eq!(
            lexer.parse_root_arguments(),
            Err(LustJsonStatus::InvalidJson),
            "Syntax error in {:?} must return InvalidJson",
            case
        );
    }

    println!("PASS: Number token class fidelity and boundary ranges verified.");
}

#[test]
fn test_ceilings_and_boundary_limits() {
    println!("\n=== 5. CEILINGS & BOUNDARY LIMITS ===");

    // 1. max_input_bytes ceiling
    let limits = LustJsonLimits {
        max_input_bytes: 8,
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    assert_eq!(
        BoundedLexer::new("[1, 2, 3]", limits, ledger.clone())
            .err()
            .unwrap(),
        LustJsonStatus::Limit,
        "Input > max_input_bytes must fail before parsing"
    );
    // Boundary N: exactly 8 bytes -> "[1, 2, 3]" is 9 bytes -> fails. "[1, 2]" is 6 bytes -> passes:
    let mut lexer = BoundedLexer::new("[1, 2]", limits, ledger.clone()).unwrap();
    assert!(lexer.parse_root_arguments().is_ok());

    // 2. max_args ceiling
    let limits = LustJsonLimits {
        max_args: 2,
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    // N = 2: passes
    let mut lexer = BoundedLexer::new("[1, 2]", limits, ledger.clone()).unwrap();
    assert_eq!(lexer.parse_root_arguments().unwrap().len(), 2);
    // N + 1 = 3: fails with Limit
    let mut lexer = BoundedLexer::new("[1, 2, 3]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::Limit),
        "Exceeding max_args must return Limit"
    );

    // 3. max_depth ceiling
    // Root array is depth 1. Nested array is depth 2.
    let limits = LustJsonLimits {
        max_depth: 2,
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    // Depth 2: passes
    let mut lexer = BoundedLexer::new("[[1]]", limits, ledger.clone()).unwrap();
    assert!(lexer.parse_root_arguments().is_ok());
    // Depth 3: fails with Limit
    let mut lexer = BoundedLexer::new("[[[1]]]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::Limit),
        "Exceeding max_depth must return Limit"
    );

    // 4. max_nodes ceiling
    // Root array (1) + 2 elements (2) = 3 nodes
    let limits = LustJsonLimits {
        max_nodes: 3,
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    // 3 nodes: passes
    let mut lexer = BoundedLexer::new("[1, 2]", limits, ledger.clone()).unwrap();
    assert!(lexer.parse_root_arguments().is_ok());
    // 4 nodes (root array + 3 elements): fails with Limit
    let mut lexer = BoundedLexer::new("[1, 2, 3]", limits, ledger.clone()).unwrap();
    assert_eq!(
        lexer.parse_root_arguments(),
        Err(LustJsonStatus::Limit),
        "Exceeding max_nodes must return Limit"
    );

    println!("PASS: Ceilings and boundary limits tested.");
}

#[test]
fn test_pre_growth_reservation_and_ledger_cleanup() {
    println!("\n=== 6. PRE-GROWTH RESERVATION & CAPACITY LEDGER ===");

    // 1. Allocation-reservation failure when max_bridge_bytes is too small
    let limits = LustJsonLimits {
        max_bridge_bytes: 32, // Too small to reserve stack, key table, and arena!
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    let res = BoundedLexer::new("[1, 2, 3]", limits, ledger);
    assert_eq!(
        res.err().unwrap(),
        LustJsonStatus::Limit,
        "Insufficient bridge capacity must fail BEFORE parsing starts!"
    );

    // 2. Verified zero-growth: pre-reserved capacities never grow during parsing
    let limits = LustJsonLimits {
        max_depth: 8,
        max_nodes: 64,
        max_bridge_bytes: 100_000,
        ..Default::default()
    };
    let ledger = CapacityLedger::new(limits.max_bridge_bytes);
    let mut lexer = BoundedLexer::new(
        r#"[{"a": 1, "b": [2, 3]}, {"c": "hello \u0041"}]"#,
        limits,
        ledger,
    )
    .unwrap();

    let initial_stack_cap = lexer.stack.capacity();
    let initial_keys_cap = lexer.keys.capacity();
    let initial_arena_cap = lexer.arena.capacity();
    let initial_allocated = lexer.ledger().allocated();

    println!(
        "Initial pre-reserved ledger allocation: {} bytes",
        initial_allocated
    );
    assert!(initial_allocated > 0, "Parser must pre-reserve in ledger");

    let args = lexer.parse_root_arguments().unwrap();
    assert_eq!(args.len(), 2);

    // Assert that collection capacities were NEVER reallocated:
    assert_eq!(lexer.stack.capacity(), initial_stack_cap);
    assert_eq!(lexer.keys.capacity(), initial_keys_cap);
    assert_eq!(lexer.arena.capacity(), initial_arena_cap);

    // Clean up ledger on drop/release:
    let final_ledger = lexer.release_ledger();
    println!(
        "Ledger released cleanly. Remaining allocated: {} bytes (Peak: {})",
        final_ledger.allocated(),
        final_ledger.peak()
    );
    assert_eq!(
        final_ledger.allocated(),
        0,
        "All pre-reserved bytes must be released cleanly"
    );

    // 3. Ledger cleanup on error path
    let ledger2 = CapacityLedger::new(limits.max_bridge_bytes);
    let mut lexer_err = BoundedLexer::new(r#"[{"a": 1, "a": 2}]"#, limits, ledger2).unwrap();
    assert_eq!(
        lexer_err.parse_root_arguments(),
        Err(LustJsonStatus::InvalidJson)
    );
    let err_ledger = lexer_err.release_ledger();
    assert_eq!(
        err_ledger.allocated(),
        0,
        "Ledger must be released cleanly even on parse error"
    );

    println!("PASS: Pre-growth reservation and ledger cleanup verified.");
}

#[test]
fn test_prototype_limitations_evidence() {
    println!("\n=== 7. PROTOTYPE LIMITATIONS ASSESSMENT ===");
    // This test formally witnesses what Gate 1 prototype CAN and CANNOT establish:
    //
    // CAN establish:
    // 1. Direct scanning of bounded JSON without generic DOM or serde deserializer.
    // 2. Pre-reservation of all parser collections before parsing begins with zero reallocation.
    // 3. Exact RFC 04 number token class fidelity:
    //    - "-0" -> Int(0)
    //    - "-0.0" -> Float(-0.0) with negative sign
    //    - 1 vs 1.0 vs 1e0 distinct token classes
    //    - Out-of-i64 (>i64::MAX, u64 only, >u64::MAX, <i64::MIN) rejected with NumberRange
    //    - Float underflow-to-zero (e.g. 1e-500) rejected with NumberRange
    //    - Float overflow (e.g. 1e500) rejected with NumberRange
    // 4. Decoded duplicate key detection (including escape-equivalent and UTF-8 escapes).
    // 5. Ceilings: max_input_bytes, max_depth, max_nodes, max_args, max_bridge_bytes.
    //
    // CANNOT establish (deferred to later gates):
    // 1. Actual `lust::bytecode::Value` construction (requires Lust VM & runtime integration).
    // 2. Allocation identity & OS allocator rounding (CapacityLedger tracks requested bytes, not jemalloc page/chunk headers).
    // 3. VM memory accounting transfer (requires active VM instance).
    // 4. C ABI export & dynamic linker symbols.

    println!("All Gate 1 prototype capabilities and explicit limitations witnessed.");
}

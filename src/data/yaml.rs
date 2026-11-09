use crate::data::{DataValue, Document, ExactNumber};
use std::collections::HashMap;
use std::fmt;

pub const MAX_YAML_BYTES: usize = 33_554_432; // 32 MiB
pub const MAX_YAML_NODES: usize = 1_000_000;
pub const MAX_EXPANSION_RATIO: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub enum YamlError {
    UnexpectedEof,
    InvalidIndentation(usize),
    DuplicateKey(String),
    CyclicReference(String),
    ForwardReference(String),
    UnsupportedTag(String),
    MaxBytesExceeded(usize),
    MaxNodesExceeded(usize),
    AmplificationLimitExceeded { b0: usize, bexp: usize, ratio: f64 },
    NodeAmplificationLimitExceeded { n0: usize, nexp: usize, ratio: f64 },
    SyntaxError(String),
}

impl fmt::Display for YamlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            YamlError::UnexpectedEof => write!(f, "Unexpected end of YAML input"),
            YamlError::InvalidIndentation(line) => write!(f, "Invalid indentation at line {}", line),
            YamlError::DuplicateKey(k) => write!(f, "Duplicate key '{}' rejected in YAML map", k),
            YamlError::CyclicReference(a) => write!(f, "Cyclic alias reference to anchor '&{}' rejected", a),
            YamlError::ForwardReference(a) => write!(f, "Forward alias reference to undeclared anchor '&{}' rejected", a),
            YamlError::UnsupportedTag(tag) => write!(f, "Arbitrary YAML tag '{}' rejected by LustSafeYaml12", tag),
            YamlError::MaxBytesExceeded(b) => write!(f, "YAML document bytes {} exceeded maximum {}", b, MAX_YAML_BYTES),
            YamlError::MaxNodesExceeded(n) => write!(f, "YAML document nodes {} exceeded maximum {}", n, MAX_YAML_NODES),
            YamlError::AmplificationLimitExceeded { b0, bexp, ratio } => {
                write!(f, "YAML expansion ratio {:.1}x (B0={}, Bexp={}) exceeded maximum 10.0x cap", ratio, b0, bexp)
            }
            YamlError::NodeAmplificationLimitExceeded { n0, nexp, ratio } => {
                write!(f, "YAML node expansion ratio {:.1}x (N0={}, Nexp={}) exceeded maximum 10.0x cap", ratio, n0, nexp)
            }
            YamlError::SyntaxError(msg) => write!(f, "YAML syntax error: {}", msg),
        }
    }
}

impl std::error::Error for YamlError {}

#[derive(Debug, Clone)]
enum RawYamlNode {
    Scalar(String),
    Sequence(Vec<RawYamlNode>),
    Mapping(Vec<(String, RawYamlNode)>),
    Alias(String),
}

pub struct LustSafeYaml12 {
    allow_aliases: bool,
    anchors: HashMap<String, RawYamlNode>,
}

impl LustSafeYaml12 {
    pub fn new() -> Self {
        Self {
            allow_aliases: false,
            anchors: HashMap::new(),
        }
    }

    pub fn with_opt_in_aliases(mut self) -> Self {
        self.allow_aliases = true;
        self
    }

    pub fn parse(&mut self, input: &str) -> Result<DataValue, YamlError> {
        self.anchors.clear();
        if input.len() > MAX_YAML_BYTES {
            return Err(YamlError::MaxBytesExceeded(input.len()));
        }

        // Basic line-based YAML parser supporting Safe profile, anchors, aliases, maps, sequences
        let lines: Vec<&str> = input.lines().collect();
        let mut idx = 0;
        let raw_root = self.parse_lines(&lines, &mut idx, 0)?;

        while idx < lines.len() {
            let trimmed = lines[idx].trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                return Err(YamlError::SyntaxError(format!("unconsumed input: {}", lines[idx])));
            }
            idx += 1;
        }

        // Pre-check DAG expansion weights before materialization
        let (b0, n0) = self.calculate_base_weights(&raw_root);
        let b0 = b0.max(1);
        let n0 = n0.max(1);

        let (bexp, nexp) = self.calculate_expanded_weights(&raw_root, &mut Vec::new())?;

        let byte_ratio = bexp as f64 / b0 as f64;
        if byte_ratio > MAX_EXPANSION_RATIO {
            return Err(YamlError::AmplificationLimitExceeded {
                b0,
                bexp,
                ratio: byte_ratio,
            });
        }

        let node_ratio = nexp as f64 / n0 as f64;
        if node_ratio > MAX_EXPANSION_RATIO {
            return Err(YamlError::NodeAmplificationLimitExceeded {
                n0,
                nexp,
                ratio: node_ratio,
            });
        }

        // Materialize into DataValue
        self.materialize_node(&raw_root)
    }

    fn calculate_base_weights(&self, node: &RawYamlNode) -> (usize, usize) {
        match node {
            RawYamlNode::Scalar(s) => (s.len().max(1), 1),
            RawYamlNode::Alias(_) => (0, 0), // Aliases do not count towards base definitions
            RawYamlNode::Sequence(items) => {
                let mut b = 0;
                let mut n = 1;
                for item in items {
                    let (ib, in_) = self.calculate_base_weights(item);
                    b += ib;
                    n += in_;
                }
                (b, n)
            }
            RawYamlNode::Mapping(entries) => {
                let mut b = 0;
                let mut n = 1;
                for (k, v) in entries {
                    b += k.len().max(1);
                    let (vb, vn) = self.calculate_base_weights(v);
                    b += vb;
                    n += vn;
                }
                (b, n)
            }
        }
    }

    fn calculate_expanded_weights(
        &self,
        node: &RawYamlNode,
        visiting: &mut Vec<String>,
    ) -> Result<(usize, usize), YamlError> {
        match node {
            RawYamlNode::Scalar(s) => Ok((s.len().max(1), 1)),
            RawYamlNode::Alias(name) => {
                if !self.allow_aliases {
                    return Err(YamlError::SyntaxError(format!("Alias *{} rejected; aliases disabled by default", name)));
                }
                if visiting.contains(name) {
                    return Err(YamlError::CyclicReference(name.clone()));
                }
                let target = self.anchors.get(name).ok_or_else(|| YamlError::ForwardReference(name.clone()))?;
                visiting.push(name.clone());
                let res = self.calculate_expanded_weights(target, visiting);
                visiting.pop();
                res
            }
            RawYamlNode::Sequence(items) => {
                let mut b = 0;
                let mut n = 1;
                for item in items {
                    let (ib, in_) = self.calculate_expanded_weights(item, visiting)?;
                    b += ib;
                    n += in_;
                }
                Ok((b, n))
            }
            RawYamlNode::Mapping(entries) => {
                let mut b = 0;
                let mut n = 1;
                for (k, v) in entries {
                    b += k.len().max(1);
                    let (vb, vn) = self.calculate_expanded_weights(v, visiting)?;
                    b += vb;
                    n += vn;
                }
                Ok((b, n))
            }
        }
    }

    fn materialize_node(&self, node: &RawYamlNode) -> Result<DataValue, YamlError> {
        match node {
            RawYamlNode::Scalar(s) => Ok(parse_yaml_scalar(s)),
            RawYamlNode::Alias(name) => {
                let target = self.anchors.get(name).ok_or_else(|| YamlError::ForwardReference(name.clone()))?;
                self.materialize_node(target)
            }
            RawYamlNode::Sequence(items) => {
                let mut arr = Vec::with_capacity(items.len());
                for item in items {
                    arr.push(self.materialize_node(item)?);
                }
                Ok(DataValue::Array(arr))
            }
            RawYamlNode::Mapping(entries) => {
                let mut doc = Document::new();
                for (k, v) in entries {
                    let val = self.materialize_node(v)?;
                    if let Err(_) = doc.try_insert(k.clone(), val) {
                        return Err(YamlError::DuplicateKey(k.clone()));
                    }
                }
                Ok(DataValue::Document(doc))
            }
        }
    }

    fn parse_lines(&mut self, lines: &[&str], idx: &mut usize, _min_indent: usize) -> Result<RawYamlNode, YamlError> {
        // Skip empty lines and comments
        while *idx < lines.len() {
            let line = lines[*idx];
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                *idx += 1;
                continue;
            }
            break;
        }

        if *idx >= lines.len() {
            return Ok(RawYamlNode::Scalar("null".to_string()));
        }

        let first_line = lines[*idx];
        let indent = get_indent(first_line);
        let trimmed = first_line.trim();

        if trimmed.starts_with('!') {
            return Err(YamlError::UnsupportedTag(trimmed.to_string()));
        }

        if trimmed.starts_with('-') {
            // Sequence
            let mut items = Vec::new();
            while *idx < lines.len() {
                let line = lines[*idx];
                let line_trimmed = line.trim();
                if line_trimmed.is_empty() || line_trimmed.starts_with('#') {
                    *idx += 1;
                    continue;
                }
                let cur_indent = get_indent(line);
                if cur_indent < indent {
                    break;
                }
                if !line_trimmed.starts_with('-') {
                    break;
                }

                let content = line_trimmed[1..].trim();
                *idx += 1;
                if content.is_empty() {
                    let item = self.parse_lines(lines, idx, indent + 2)?;
                    items.push(item);
                } else {
                    items.push(self.parse_single_value(content)?);
                }
            }
            Ok(RawYamlNode::Sequence(items))
        } else if trimmed.contains(':') && !trimmed.starts_with('"') && !trimmed.starts_with('\'') {
            // Mapping
            let mut entries = Vec::new();
            while *idx < lines.len() {
                let line = lines[*idx];
                let line_trimmed = line.trim();
                if line_trimmed.is_empty() || line_trimmed.starts_with('#') {
                    *idx += 1;
                    continue;
                }
                let cur_indent = get_indent(line);
                if cur_indent < indent {
                    break;
                }

                if let Some(colon_pos) = line_trimmed.find(':') {
                    let raw_key = line_trimmed[..colon_pos].trim();
                    let raw_val = line_trimmed[colon_pos + 1..].trim();
                    let key = strip_quotes(raw_key);

                    *idx += 1;
                    let val_node = if raw_val.is_empty() {
                        self.parse_lines(lines, idx, indent + 2)?
                    } else {
                        self.parse_single_value(raw_val)?
                    };

                    entries.push((key, val_node));
                } else {
                    break;
                }
            }
            Ok(RawYamlNode::Mapping(entries))
        } else {
            *idx += 1;
            self.parse_single_value(trimmed)
        }
    }

    fn parse_single_value(&mut self, val_str: &str) -> Result<RawYamlNode, YamlError> {
        let trimmed = val_str.trim();

        if trimmed.starts_with('!') {
            return Err(YamlError::UnsupportedTag(trimmed.to_string()));
        }

        if trimmed.starts_with('&') {
            // Anchor definition: &anchor_name value
            let parts: Vec<&str> = trimmed[1..].splitn(2, ' ').collect();
            let anchor_name = parts[0].trim().to_string();
            let rest = if parts.len() > 1 { parts[1].trim() } else { "" };
            let inner_node = self.parse_single_value(rest)?;
            self.anchors.insert(anchor_name, inner_node.clone());
            Ok(inner_node)
        } else if trimmed.starts_with('*') {
            // Alias reference: *anchor_name
            let alias_name = trimmed[1..].trim().to_string();
            if !self.anchors.contains_key(&alias_name) {
                return Err(YamlError::ForwardReference(alias_name));
            }
            Ok(RawYamlNode::Alias(alias_name))
        } else {
            Ok(RawYamlNode::Scalar(trimmed.to_string()))
        }
    }
}

fn get_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
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

fn parse_yaml_scalar(s: &str) -> DataValue {
    let clean = strip_quotes(s);
    match clean.as_str() {
        "null" | "~" => DataValue::Null,
        "true" | "True" | "TRUE" => DataValue::Bool(true),
        "false" | "False" | "FALSE" => DataValue::Bool(false),
        other => {
            if let Ok(i) = other.parse::<i64>() {
                DataValue::Number(ExactNumber::Int(i))
            } else if let Ok(f) = other.parse::<f64>() {
                DataValue::Number(ExactNumber::Float(f))
            } else {
                DataValue::String(clean)
            }
        }
    }
}

pub fn from_str(input: &str) -> Result<DataValue, YamlError> {
    let mut parser = LustSafeYaml12::new();
    parser.parse(input)
}

pub fn from_str_with_aliases(input: &str) -> Result<DataValue, YamlError> {
    let mut parser = LustSafeYaml12::new().with_opt_in_aliases();
    parser.parse(input)
}

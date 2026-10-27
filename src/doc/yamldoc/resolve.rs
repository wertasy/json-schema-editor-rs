//! Scalar resolution (yaml.v3 resolve.go) and numeric normalization
//! (yamldoc.go normalizeNum).

use crate::doc::DocError;

/// The resolved yaml.v3 short tag of a plain scalar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlainTag {
    Null,
    Bool,
    Int,
    Float,
    Timestamp,
    Merge,
    Str,
}

fn resolve_map(in_: &str) -> Option<PlainTag> {
    match in_ {
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" => Some(PlainTag::Bool),
        "" | "~" | "null" | "Null" | "NULL" => Some(PlainTag::Null),
        ".nan" | ".NaN" | ".NAN" | ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF"
        | "-.inf" | "-.Inf" | "-.INF" => Some(PlainTag::Float),
        "<<" => Some(PlainTag::Merge),
        _ => None,
    }
}

/// yamlStyleFloat: `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`
fn yaml_style_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    } else {
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

/// parseTimestamp: YYYY- prefix check, then the four allowed layouts
/// (RFC3339Nano with short date fields, its lower-case-t variant, space
/// separated without zone, and date only).
fn parse_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i != 4 || i == b.len() || b[i] != b'-' {
        return false;
    }
    let num = |i: &mut usize, min: usize, max: usize| -> bool {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        let n = *i - start;
        (min..=max).contains(&n)
    };
    // -M-D (1-2 digit fields)
    i += 1;
    if !num(&mut i, 1, 2) {
        return false;
    }
    if b.get(i) != Some(&b'-') {
        return false;
    }
    i += 1;
    if !num(&mut i, 1, 2) {
        return false;
    }
    if i == b.len() {
        return true; // date only
    }
    let sep = b[i];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return false;
    }
    i += 1;
    // H:M:S
    if !num(&mut i, 1, 2) || b.get(i) != Some(&b':') {
        return false;
    }
    i += 1;
    if !num(&mut i, 1, 2) || b.get(i) != Some(&b':') {
        return false;
    }
    i += 1;
    if !num(&mut i, 1, 2) {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !num(&mut i, 1, 9) {
            return false;
        }
    }
    if sep == b' ' {
        return i == b.len(); // no zone in the space layout
    }
    // Z07:00: Z or ±hh:mm
    match b.get(i) {
        Some(b'Z') => i + 1 == b.len(),
        Some(c) if *c == b'+' || *c == b'-' => {
            i += 1;
            if !num(&mut i, 2, 2) || b.get(i) != Some(&b':') {
                return false;
            }
            i += 1;
            if !num(&mut i, 2, 2) {
                return false;
            }
            i == b.len()
        }
        _ => false,
    }
}

/// resolve of a plain scalar (yaml.v3): hint-dispatched.
pub(crate) fn resolve_plain(in_: &str) -> PlainTag {
    if let Some(t) = resolve_map(in_) {
        return t;
    }
    let hint = match in_.as_bytes().first() {
        None => b'N',
        Some(c) => match c {
            b'+' | b'-' => b'S',
            b'0'..=b'9' => b'D',
            b'y' | b'Y' | b'n' | b'N' | b't' | b'T' | b'f' | b'F' | b'o' | b'O' | b'~' => b'M',
            b'.' => b'.',
            _ => 0,
        },
    };
    if hint == 0 {
        return PlainTag::Str;
    }
    match hint {
        // map misses only (v3 dropped the y/n/on/off booleans)
        b'M' => PlainTag::Str,
        b'.' => {
            if in_.parse::<f64>().is_ok() {
                PlainTag::Float
            } else {
                PlainTag::Str
            }
        }
        b'D' | b'S' => {
            if parse_timestamp(in_) {
                return PlainTag::Timestamp;
            }
            let plain: String = in_.replace('_', "");
            if parse_int_str(&plain).is_some() || parse_uint_str(&plain).is_some() {
                return PlainTag::Int;
            }
            if yaml_style_float(&plain) && plain.parse::<f64>().is_ok() {
                return PlainTag::Float;
            }
            if let Some(rest) = plain.strip_prefix("0b") {
                if i64::from_str_radix(rest, 2).is_ok() || u64::from_str_radix(rest, 2).is_ok() {
                    return PlainTag::Int;
                }
            } else if let Some(rest) = plain.strip_prefix("-0b") {
                if i64::from_str_radix(&format!("-{rest}"), 2).is_ok() {
                    return PlainTag::Int;
                }
            }
            if let Some(rest) = plain.strip_prefix("0o") {
                if i64::from_str_radix(rest, 8).is_ok() || u64::from_str_radix(rest, 8).is_ok() {
                    return PlainTag::Int;
                }
            } else if let Some(rest) = plain.strip_prefix("-0o") {
                if i64::from_str_radix(&format!("-{rest}"), 8).is_ok() {
                    return PlainTag::Int;
                }
            }
            PlainTag::Str
        }
        _ => PlainTag::Str,
    }
}

fn split_prefix(rest: &str) -> (&str, u32) {
    if let Some(r) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        (r, 16)
    } else if let Some(r) = rest.strip_prefix("0b").or_else(|| rest.strip_prefix("0B")) {
        (r, 2)
    } else if let Some(r) = rest.strip_prefix("0o").or_else(|| rest.strip_prefix("0O")) {
        (r, 8)
    } else if rest.len() > 1 && rest.starts_with('0') {
        (&rest[1..], 8)
    } else {
        (rest, 10)
    }
}

/// strconv.ParseInt(s, 0, 64).
pub(crate) fn parse_int_str(s: &str) -> Option<i64> {
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if rest.is_empty() {
        return None;
    }
    let (digits, radix) = split_prefix(rest);
    if digits.is_empty() {
        return None;
    }
    let v = i64::from_str_radix(digits, radix).ok()?;
    Some(if neg { -v } else { v })
}

/// strconv.ParseUint(s, 0, 64).
fn parse_uint_str(s: &str) -> Option<u64> {
    let rest = s.strip_prefix('+').unwrap_or(s);
    if rest.starts_with('-') || rest.is_empty() {
        return None;
    }
    let (digits, radix) = split_prefix(rest);
    if digits.is_empty() {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

/// jsonNumber: `^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?$`
fn is_json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    if i >= b.len() {
        return false;
    }
    if b[i] == b'0' {
        i += 1;
    } else if b[i].is_ascii_digit() {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    i == b.len()
}

/// normalizeNum turns a YAML numeric literal into a JSON numeric literal:
/// valid JSON passes through; ints decode via Go's base-0 parsing; floats
/// use the ECMAScript shortest spelling (`1e6` → `1000000`); inf/nan are refused.
pub(crate) fn normalize_num(tag: &str, raw: &str) -> Result<String, DocError> {
    if is_json_number(raw) {
        return Ok(raw.to_string());
    }
    let clean: String = raw.replace('_', "");
    match tag {
        "!!int" => {
            if let Some(i) = parse_int_str(&clean) {
                return Ok(i.to_string());
            }
            if let Some(u) = parse_uint_str(&clean) {
                return Ok(u.to_string());
            }
            Err(DocError {
                msg: format!("unsupported YAML integer {raw:?}"),
            })
        }
        "!!float" => {
            let mag = clean.trim_start_matches(['+', '-']).to_ascii_lowercase();
            if mag == ".inf" || mag == ".nan" || mag == "inf" || mag == "nan" {
                return Err(DocError {
                    msg: format!(
                        "unsupported YAML float {raw:?}: inf/nan are not representable in JSON"
                    ),
                });
            }
            match clean.parse::<f64>() {
                Ok(f) => Ok(crate::doc::jsondoc::number_text(f)),
                Err(_) => Err(DocError {
                    msg: format!("unsupported YAML float {raw:?}"),
                }),
            }
        }
        _ => Ok(raw.to_string()),
    }
}

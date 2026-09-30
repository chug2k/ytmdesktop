//! The small subset of CBOR (RFC 8949) that caBLE and CTAP2 use: definite
//! lengths, integers, byte and text strings, arrays, maps, and booleans.
//!
//! The encoder writes maps in the order given. CTAP2 needs canonical key
//! order, so callers build maps with their keys already sorted.

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Bool(bool),
    Null,
}

impl Value {
    pub fn text(s: &str) -> Self {
        Value::Text(s.to_string())
    }

    /// Look up `key` in a map. `None` for a missing key or a non-map.
    pub fn get(&self, key: &Value) -> Option<&Value> {
        match self {
            Value::Map(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_int(&self, key: i64) -> Option<&Value> {
        self.get(&Value::Int(key))
    }

    pub fn get_text(&self, key: &str) -> Option<&Value> {
        self.get(&Value::text(key))
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, value);
    out
}

fn write_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let major = major << 5;
    match n {
        0..=23 => out.push(major | n as u8),
        24..=0xff => out.extend_from_slice(&[major | 24, n as u8]),
        0x100..=0xffff => {
            out.push(major | 25);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(major | 26);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(major | 27);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

fn write(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Int(i) if *i >= 0 => write_head(out, 0, *i as u64),
        Value::Int(i) => write_head(out, 1, (-1 - *i) as u64),
        Value::Bytes(b) => {
            write_head(out, 2, b.len() as u64);
            out.extend_from_slice(b);
        }
        Value::Text(s) => {
            write_head(out, 3, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            write_head(out, 4, items.len() as u64);
            items.iter().for_each(|item| write(out, item));
        }
        Value::Map(entries) => {
            write_head(out, 5, entries.len() as u64);
            for (k, v) in entries {
                write(out, k);
                write(out, v);
            }
        }
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Null => out.push(0xf6),
    }
}

/// Nesting limit. Real CTAP2 messages nest three or four levels; the limit
/// stops a hostile peer from exhausting the stack.
const MAX_DEPTH: usize = 16;

/// Decode the first CBOR item in `input`. Trailing bytes are ignored: caBLE
/// v2.0 pads its post-handshake message after the CBOR item.
pub fn decode(input: &[u8]) -> Option<Value> {
    let mut reader = Reader { input, pos: 0 };
    reader.value(0)
}

struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let end = self.pos.checked_add(n)?;
        let slice = self.input.get(self.pos..end)?;
        self.pos = end;
        Some(slice)
    }

    fn head(&mut self) -> Option<(u8, u64)> {
        let first = *self.take(1)?.first()?;
        let (major, info) = (first >> 5, first & 0x1f);
        let n = match info {
            0..=23 => u64::from(info),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(self.take(2)?.try_into().ok()?)),
            26 => u64::from(u32::from_be_bytes(self.take(4)?.try_into().ok()?)),
            27 => u64::from_be_bytes(self.take(8)?.try_into().ok()?),
            // Indefinite lengths and reserved values are not used by CTAP2.
            _ => return None,
        };
        Some((major, n))
    }

    /// A length prefix, checked against the bytes that remain, so a forged
    /// length cannot trigger a huge allocation.
    fn len(&self, n: u64) -> Option<usize> {
        let n = usize::try_from(n).ok()?;
        (n <= self.input.len() - self.pos).then_some(n)
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        let (major, n) = self.head()?;
        Some(match major {
            0 => Value::Int(i64::try_from(n).ok()?),
            1 => Value::Int(-1 - i64::try_from(n).ok()?),
            2 => {
                let len = self.len(n)?;
                Value::Bytes(self.take(len)?.to_vec())
            }
            3 => {
                let len = self.len(n)?;
                Value::Text(String::from_utf8(self.take(len)?.to_vec()).ok()?)
            }
            4 => {
                let count = self.len(n)?;
                let items = (0..count)
                    .map(|_| self.value(depth + 1))
                    .collect::<Option<Vec<_>>>()?;
                Value::Array(items)
            }
            5 => {
                let count = self.len(n)?;
                let entries = (0..count)
                    .map(|_| Some((self.value(depth + 1)?, self.value(depth + 1)?)))
                    .collect::<Option<Vec<_>>>()?;
                Value::Map(entries)
            }
            7 => match n {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                _ => return None,
            },
            // Tags (6) are not used by CTAP2.
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_rfc_8949_examples() {
        assert_eq!(encode(&Value::Int(0)), [0x00]);
        assert_eq!(encode(&Value::Int(23)), [0x17]);
        assert_eq!(encode(&Value::Int(24)), [0x18, 0x18]);
        assert_eq!(encode(&Value::Int(1000)), [0x19, 0x03, 0xe8]);
        assert_eq!(
            encode(&Value::Int(1_000_000)),
            [0x1a, 0x00, 0x0f, 0x42, 0x40]
        );
        assert_eq!(encode(&Value::Int(-1)), [0x20]);
        assert_eq!(encode(&Value::Int(-1000)), [0x39, 0x03, 0xe7]);
        assert_eq!(encode(&Value::text("IETF")), b"\x64IETF");
        assert_eq!(encode(&Value::Bool(true)), [0xf5]);
        assert_eq!(
            encode(&Value::Map(vec![
                (Value::Int(1), Value::Int(2)),
                (Value::Int(3), Value::Int(4))
            ])),
            [0xa2, 0x01, 0x02, 0x03, 0x04]
        );
    }

    #[test]
    fn round_trips_nested_values() {
        let value = Value::Map(vec![
            (Value::Int(1), Value::Bytes(vec![0xaa; 300])),
            (
                Value::text("list"),
                Value::Array(vec![Value::Null, Value::Bool(false), Value::Int(-7)]),
            ),
        ]);
        assert_eq!(decode(&encode(&value)), Some(value));
    }

    #[test]
    fn ignores_trailing_padding() {
        assert_eq!(decode(&[0x01, 0x00, 0x00, 0x00]), Some(Value::Int(1)));
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(decode(&[]), None);
        // Byte string claims 4 GiB.
        assert_eq!(decode(&[0x5a, 0xff, 0xff, 0xff, 0xff, 0x00]), None);
        // Array claims more items than there are bytes.
        assert_eq!(decode(&[0x9a, 0x00, 0x10, 0x00, 0x00]), None);
        // Indefinite-length array.
        assert_eq!(decode(&[0x9f, 0x01, 0xff]), None);
        // Invalid UTF-8 in a text string.
        assert_eq!(decode(&[0x61, 0xff]), None);
        // Nesting past the limit.
        assert_eq!(decode(&[0x81; 64]), None);
    }
}

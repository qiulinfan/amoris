//! JSON Pointers (RFC 6901) into a request: where a problem is (shared/contract/errors.md, `path`).

use std::cmp::Ordering;
use std::fmt;

/// One step of a pointer: an object key or an array index.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Segment {
    Key(String),
    Index(usize),
}

/// A JSON Pointer as segments. `Pointer::root()` is the whole request (written `""`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pointer {
    segments: Vec<Segment>,
}

impl Pointer {
    /// The whole document.
    pub fn root() -> Pointer {
        Pointer::default()
    }

    /// Parses `"/actions/0/params"`. A segment of decimal digits without a leading zero is read as
    /// an index (it may also be a key; ordering treats it as a number either way, errors.md,
    /// Several problems). `~1` and `~0` unescape to `/` and `~`.
    pub fn parse(text: &str) -> Pointer {
        let mut segments = Vec::new();
        if !text.is_empty() {
            for raw in text.strip_prefix('/').unwrap_or(text).split('/') {
                let key = raw.replace("~1", "/").replace("~0", "~");
                segments.push(match parse_index(&key) {
                    Some(i) => Segment::Index(i),
                    None => Segment::Key(key),
                });
            }
        }
        Pointer { segments }
    }

    /// This pointer with an object key appended.
    pub fn key(&self, key: &str) -> Pointer {
        let mut p = self.clone();
        p.segments.push(Segment::Key(key.to_owned()));
        p
    }

    /// This pointer with an array index appended.
    pub fn index(&self, index: usize) -> Pointer {
        let mut p = self.clone();
        p.segments.push(Segment::Index(index));
        p
    }

    /// The pointer one level up; the root's parent is the root.
    pub fn parent(&self) -> Pointer {
        let mut p = self.clone();
        p.segments.pop();
        p
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// The last segment, if any.
    pub fn last(&self) -> Option<&Segment> {
        self.segments.last()
    }

    /// The `{field}` placeholder (errors.md, Placeholders): the last key, or `item <n>` for an index.
    pub fn field_name(&self) -> String {
        match self.segments.last() {
            Some(Segment::Key(k)) => k.clone(),
            Some(Segment::Index(i)) => format!("item {i}"),
            None => String::new(),
        }
    }

    /// The path as an agent would write it in prose: `actions[0].params`.
    pub fn dotted(&self) -> String {
        let mut out = String::new();
        for s in &self.segments {
            match s {
                Segment::Key(k) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                Segment::Index(i) => {
                    out.push('[');
                    out.push_str(&i.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}

fn parse_index(s: &str) -> Option<usize> {
    let digits = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if digits && (s == "0" || !s.starts_with('0')) {
        s.parse().ok()
    } else {
        None
    }
}

impl fmt::Display for Pointer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for s in &self.segments {
            f.write_str("/")?;
            match s {
                Segment::Key(k) => f.write_str(&k.replace('~', "~0").replace('/', "~1"))?,
                Segment::Index(i) => write!(f, "{i}")?,
            }
        }
        Ok(())
    }
}

/// Segment by segment: indices as numbers, keys by bytes, an index before a key; a prefix first.
impl Ord for Pointer {
    fn cmp(&self, other: &Pointer) -> Ordering {
        for (a, b) in self.segments.iter().zip(&other.segments) {
            let o = match (a, b) {
                (Segment::Index(x), Segment::Index(y)) => x.cmp(y),
                (Segment::Key(x), Segment::Key(y)) => x.as_bytes().cmp(y.as_bytes()),
                (Segment::Index(_), Segment::Key(_)) => Ordering::Less,
                (Segment::Key(_), Segment::Index(_)) => Ordering::Greater,
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        self.segments.len().cmp(&other.segments.len())
    }
}

impl PartialOrd for Pointer {
    fn partial_cmp(&self, other: &Pointer) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_escapes() {
        let p = Pointer::root().key("actions").index(0).key("a/b~c");
        assert_eq!(p.to_string(), "/actions/0/a~1b~0c");
        assert_eq!(Pointer::parse(&p.to_string()), p);
        assert_eq!(Pointer::parse("").to_string(), "");
        assert_eq!(p.dotted(), "actions[0].a/b~c");
        assert_eq!(Pointer::parse("/actions/3").field_name(), "item 3");
    }

    #[test]
    fn order_is_numeric_for_indices() {
        let a = Pointer::parse("/actions/2/params/x");
        let b = Pointer::parse("/actions/10/params/a");
        assert!(a < b);
        assert!(Pointer::parse("/a") < Pointer::parse("/a/b"));
        assert!(Pointer::parse("/a/b") < Pointer::parse("/b"));
        assert_eq!(
            Pointer::parse("/01").segments(),
            [Segment::Key("01".into())]
        );
    }
}

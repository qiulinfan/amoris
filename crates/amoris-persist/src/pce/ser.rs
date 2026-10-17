//! The PCE encoder (docs/spec/persistence.md 3.1 to 3.5).

use serde::ser::{self, Serialize};

use super::{MAX_DEPTH, PceError, write_uleb};

/// Writes PCE onto a buffer.
pub struct Encoder<'a> {
    out: &'a mut Vec<u8>,
    finite: bool,
    /// Compound values open around the next one, counted as the decoder counts them.
    depth: u32,
}

impl<'a> Encoder<'a> {
    /// With `finite`, NaN and infinities are refused (Component and Resource sections).
    pub fn new(out: &'a mut Vec<u8>, finite: bool) -> Encoder<'a> {
        Encoder {
            out,
            finite,
            depth: 0,
        }
    }

    /// Opens one more compound value; [`Compound::done`] or the caller closes it.
    fn enter(&mut self) -> Result<(), PceError> {
        if self.depth >= MAX_DEPTH {
            return Err(PceError::Encode(format!("nesting deeper than {MAX_DEPTH}")));
        }
        self.depth += 1;
        Ok(())
    }

    /// Writes `v` inside one more compound value (a newtype, `Some`, a newtype variant).
    fn inside<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.enter()?;
        v.serialize(&mut *self)?;
        self.depth -= 1;
        Ok(())
    }

    fn len(&mut self, len: Option<usize>, what: &str) -> Result<usize, PceError> {
        let len = len.ok_or_else(|| PceError::Encode(format!("a {what} without a length")))?;
        write_uleb(self.out, len as u64);
        Ok(len)
    }

    fn float_ok(&self, finite: bool) -> Result<(), PceError> {
        if self.finite && !finite {
            Err(PceError::Encode(
                "a NaN or an infinity in persisted state".into(),
            ))
        } else {
            Ok(())
        }
    }
}

/// A sequence, map or struct being written: counts what is written against what was announced, so
/// a `Serialize` that writes fewer or more elements or fields than it announced fails.
pub struct Compound<'e, 'a> {
    enc: &'e mut Encoder<'a>,
    announced: usize,
    written: usize,
    what: &'static str,
}

impl Compound<'_, '_> {
    fn item<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.written += 1;
        v.serialize(&mut *self.enc)
    }

    fn done(self) -> Result<(), PceError> {
        self.enc.depth -= 1;
        if self.written == self.announced {
            Ok(())
        } else {
            Err(PceError::Encode(format!(
                "a {} announced {} items and wrote {}",
                self.what, self.announced, self.written
            )))
        }
    }

    fn skipped(key: &'static str) -> PceError {
        PceError::Encode(format!(
            "field `{key}` is skipped (skip_serializing_if), which a format without names cannot \
             read back"
        ))
    }
}

impl<'e, 'a> ser::Serializer for &'e mut Encoder<'a> {
    type Ok = ();
    type Error = PceError;
    type SerializeSeq = Compound<'e, 'a>;
    type SerializeTuple = Compound<'e, 'a>;
    type SerializeTupleStruct = Compound<'e, 'a>;
    type SerializeTupleVariant = Compound<'e, 'a>;
    type SerializeMap = Compound<'e, 'a>;
    type SerializeStruct = Compound<'e, 'a>;
    type SerializeStructVariant = Compound<'e, 'a>;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bool(self, v: bool) -> Result<(), PceError> {
        self.out.push(u8::from(v));
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_i128(self, v: i128) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> Result<(), PceError> {
        self.out.push(v);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_u128(self, v: u128) -> Result<(), PceError> {
        self.out.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), PceError> {
        self.float_ok(v.is_finite())?;
        self.out.extend_from_slice(&v.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> Result<(), PceError> {
        self.float_ok(v.is_finite())?;
        self.out.extend_from_slice(&v.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<(), PceError> {
        self.out.extend_from_slice(&u32::from(v).to_le_bytes());
        Ok(())
    }
    fn serialize_str(self, v: &str) -> Result<(), PceError> {
        write_uleb(self.out, v.len() as u64);
        self.out.extend_from_slice(v.as_bytes());
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), PceError> {
        write_uleb(self.out, v.len() as u64);
        self.out.extend_from_slice(v);
        Ok(())
    }
    fn serialize_none(self) -> Result<(), PceError> {
        self.out.push(0);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<(), PceError> {
        self.out.push(1);
        self.inside(v)
    }
    fn serialize_unit(self) -> Result<(), PceError> {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), PceError> {
        Ok(())
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
    ) -> Result<(), PceError> {
        write_uleb(self.out, u64::from(i));
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<(), PceError> {
        self.inside(v)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        v: &T,
    ) -> Result<(), PceError> {
        write_uleb(self.out, u64::from(i));
        self.inside(v)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'e, 'a>, PceError> {
        self.enter()?;
        let announced = self.len(len, "sequence")?;
        Ok(Compound {
            enc: self,
            announced,
            written: 0,
            what: "sequence",
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'e, 'a>, PceError> {
        self.enter()?;
        Ok(Compound {
            enc: self,
            announced: len,
            written: 0,
            what: "tuple",
        })
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        len: usize,
    ) -> Result<Compound<'e, 'a>, PceError> {
        self.serialize_tuple(len)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Compound<'e, 'a>, PceError> {
        write_uleb(self.out, u64::from(i));
        self.serialize_tuple(len)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Compound<'e, 'a>, PceError> {
        self.enter()?;
        let entries = self.len(len, "map")?;
        Ok(Compound {
            enc: self,
            announced: 2 * entries,
            written: 0,
            what: "map",
        })
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Compound<'e, 'a>, PceError> {
        self.enter()?;
        Ok(Compound {
            enc: self,
            announced: len,
            written: 0,
            what: "struct",
        })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        i: u32,
        _: &'static str,
        len: usize,
    ) -> Result<Compound<'e, 'a>, PceError> {
        write_uleb(self.out, u64::from(i));
        self.serialize_struct("", len)
    }
}

impl ser::SerializeSeq for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.item(v)
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeTuple for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.item(v)
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeTupleStruct for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.item(v)
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeTupleVariant for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.item(v)
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeMap for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<(), PceError> {
        self.item(k)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), PceError> {
        self.item(v)
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeStruct for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), PceError> {
        self.item(v)
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), PceError> {
        Err(Compound::skipped(key))
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

impl ser::SerializeStructVariant for Compound<'_, '_> {
    type Ok = ();
    type Error = PceError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        v: &T,
    ) -> Result<(), PceError> {
        self.item(v)
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), PceError> {
        Err(Compound::skipped(key))
    }
    fn end(self) -> Result<(), PceError> {
        self.done()
    }
}

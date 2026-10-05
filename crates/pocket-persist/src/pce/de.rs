//! The strict PCE decoder (docs/spec/persistence.md 3.6): a `bool` other than 0 or 1, invalid
//! UTF-8, a `char` outside the scalar values, a non-minimal ULEB128, an option tag other than 0 or
//! 1, a variant index past `u32`, a length past the bytes left, and bytes left over all fail.
//! Variant indices out of range and values a type's own `Deserialize` refuses fail through the
//! type. `deserialize_any` fails: PCE is not self-describing. A value nested more than
//! [`MAX_DEPTH`] compound values deep fails too, so crafted bytes cannot exhaust the stack.

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, IntoDeserializer, Visitor};

use super::{MAX_DEPTH, PceError, read_uleb};

/// Reads PCE values one after another from a byte slice.
pub struct Decoder<'de> {
    bytes: &'de [u8],
    pos: usize,
    finite: bool,
    /// Compound values open around the next one (`super::MAX_DEPTH`).
    depth: u32,
}

impl<'de> Decoder<'de> {
    /// With `finite`, a NaN or an infinity is refused (the encoders that refuse them, 3.1).
    pub fn new(bytes: &'de [u8], finite: bool) -> Decoder<'de> {
        Decoder {
            bytes,
            pos: 0,
            finite,
            depth: 0,
        }
    }

    /// Opens one more compound value (a sequence, tuple, map, struct, newtype, `Some` or an enum
    /// variant with content, as the encoder counts them): `noncanonical` past `MAX_DEPTH`. The
    /// JSON conversion and the field diff, which read with a decoder, count on it too.
    pub(crate) fn enter(&mut self) -> Result<(), PceError> {
        if self.depth >= MAX_DEPTH {
            return Err(PceError::noncanonical(
                self.pos,
                format!("nesting deeper than {MAX_DEPTH}"),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    /// Closes what [`Decoder::enter`] opened.
    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Runs `f` inside one more compound value.
    fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, PceError>,
    ) -> Result<T, PceError> {
        self.enter()?;
        let r = f(self);
        self.leave();
        r
    }

    /// The offset of the next byte.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Moves to an offset read before (to read a value again as a whole).
    pub(crate) fn set_pos(&mut self, pos: usize) {
        self.pos = pos.min(self.bytes.len());
    }

    /// The bytes from `from` to the current offset.
    pub(crate) fn since(&self, from: usize) -> &'de [u8] {
        &self.bytes[from.min(self.pos)..self.pos]
    }

    /// Whether every byte has been read.
    pub fn at_end(&self) -> bool {
        self.pos == self.bytes.len()
    }

    /// Decodes the next value.
    pub fn value<T: Deserialize<'de>>(&mut self) -> Result<T, PceError> {
        T::deserialize(&mut *self).map_err(|e| e.at(self.pos))
    }

    /// A ULEB128 length or count.
    pub fn uleb(&mut self) -> Result<u64, PceError> {
        read_uleb(self.bytes, &mut self.pos)
    }

    /// The next `n` bytes.
    pub fn take(&mut self, n: usize) -> Result<&'de [u8], PceError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or(PceError::Truncated {
                offset: self.bytes.len(),
            })?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], PceError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// A length as `usize`, at most the bytes left (every element takes at least one byte, except
    /// zero-sized ones, which persisted types do not repeat).
    fn len(&mut self) -> Result<usize, PceError> {
        let at = self.pos;
        let n = self.uleb()?;
        usize::try_from(n)
            .ok()
            .filter(|&n| n <= self.bytes.len() - self.pos)
            .ok_or_else(|| {
                if n > (self.bytes.len() - self.pos) as u64 {
                    PceError::Truncated {
                        offset: self.bytes.len(),
                    }
                } else {
                    PceError::noncanonical(at, "a length past this platform's usize")
                }
            })
    }

    /// Fails when bytes are left over.
    pub fn finish(&self) -> Result<(), PceError> {
        if self.at_end() {
            Ok(())
        } else {
            Err(PceError::noncanonical(
                self.pos,
                format!("{} bytes left over", self.bytes.len() - self.pos),
            ))
        }
    }

    fn float_ok(&self, finite: bool, at: usize) -> Result<(), PceError> {
        if self.finite && !finite {
            Err(PceError::noncanonical(
                at,
                "a NaN or an infinity in persisted state",
            ))
        } else {
            Ok(())
        }
    }
}

impl<'de> de::Deserializer<'de> for &mut Decoder<'de> {
    type Error = PceError;

    fn is_human_readable(&self) -> bool {
        false
    }

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, PceError> {
        Err(PceError::noncanonical(
            self.pos,
            "deserialize_any: PCE is not self-describing (untagged, flatten or a tagged enum)",
        ))
    }

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let at = self.pos;
        match self.array::<1>()?[0] {
            0 => v.visit_bool(false),
            1 => v.visit_bool(true),
            b => Err(PceError::noncanonical(at, format!("a bool of {b}"))),
        }
    }

    fn deserialize_i8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_i8(i8::from_le_bytes(self.array()?))
    }
    fn deserialize_i16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_i16(i16::from_le_bytes(self.array()?))
    }
    fn deserialize_i32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_i32(i32::from_le_bytes(self.array()?))
    }
    fn deserialize_i64<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_i64(i64::from_le_bytes(self.array()?))
    }
    fn deserialize_i128<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_i128(i128::from_le_bytes(self.array()?))
    }
    fn deserialize_u8<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_u8(self.array::<1>()?[0])
    }
    fn deserialize_u16<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_u16(u16::from_le_bytes(self.array()?))
    }
    fn deserialize_u32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_u32(u32::from_le_bytes(self.array()?))
    }
    fn deserialize_u64<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_u64(u64::from_le_bytes(self.array()?))
    }
    fn deserialize_u128<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_u128(u128::from_le_bytes(self.array()?))
    }

    fn deserialize_f32<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let at = self.pos;
        let x = f32::from_bits(u32::from_le_bytes(self.array()?));
        self.float_ok(x.is_finite(), at)?;
        v.visit_f32(x)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let at = self.pos;
        let x = f64::from_bits(u64::from_le_bytes(self.array()?));
        self.float_ok(x.is_finite(), at)?;
        v.visit_f64(x)
    }

    fn deserialize_char<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let at = self.pos;
        let n = u32::from_le_bytes(self.array()?);
        let c = char::from_u32(n).ok_or_else(|| {
            PceError::noncanonical(at, format!("{n:#x} is not a Unicode scalar value"))
        })?;
        v.visit_char(c)
    }

    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let n = self.len()?;
        let at = self.pos;
        let b = self.take(n)?;
        let s = std::str::from_utf8(b).map_err(|_| PceError::noncanonical(at, "invalid UTF-8"))?;
        v.visit_borrowed_str(s)
    }
    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        self.deserialize_str(v)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let n = self.len()?;
        v.visit_borrowed_bytes(self.take(n)?)
    }
    fn deserialize_byte_buf<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        self.deserialize_bytes(v)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        let at = self.pos;
        match self.array::<1>()?[0] {
            0 => v.visit_none(),
            1 => self.nested(|d| v.visit_some(d)),
            b => Err(PceError::noncanonical(at, format!("an option tag of {b}"))),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        v.visit_unit()
    }
    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value, PceError> {
        v.visit_unit()
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value, PceError> {
        self.nested(|d| v.visit_newtype_struct(d))
    }

    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        self.nested(|d| {
            let n = d.len()?;
            v.visit_seq(Items { de: d, left: n })
        })
    }
    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, PceError> {
        self.nested(|d| v.visit_seq(Items { de: d, left: len }))
    }
    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, PceError> {
        self.nested(|d| v.visit_seq(Items { de: d, left: len }))
    }
    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, PceError> {
        self.nested(|d| {
            let n = d.len()?;
            v.visit_map(Items { de: d, left: n })
        })
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, PceError> {
        self.nested(|d| {
            v.visit_seq(Items {
                de: d,
                left: fields.len(),
            })
        })
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, PceError> {
        v.visit_enum(self)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, _: V) -> Result<V::Value, PceError> {
        Err(PceError::noncanonical(
            self.pos,
            "an identifier: PCE writes no names",
        ))
    }
    fn deserialize_ignored_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, PceError> {
        Err(PceError::noncanonical(
            self.pos,
            "a value to skip: PCE has no unknown fields",
        ))
    }
}

/// The elements of a sequence, tuple, struct or map, exactly `left` of them.
struct Items<'a, 'de> {
    de: &'a mut Decoder<'de>,
    left: usize,
}

impl<'de> de::SeqAccess<'de> for Items<'_, 'de> {
    type Error = PceError;
    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, PceError> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

impl<'de> de::MapAccess<'de> for Items<'_, 'de> {
    type Error = PceError;
    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, PceError> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, PceError> {
        seed.deserialize(&mut *self.de)
    }
    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

impl<'de> de::EnumAccess<'de> for &mut Decoder<'de> {
    type Error = PceError;
    type Variant = Self;
    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self), PceError> {
        let at = self.pos;
        let i = self.uleb()?;
        let i = u32::try_from(i)
            .map_err(|_| PceError::noncanonical(at, format!("a variant index of {i}")))?;
        let v = seed
            .deserialize(IntoDeserializer::<PceError>::into_deserializer(i))
            .map_err(|e| e.at(at))?;
        Ok((v, self))
    }
}

impl<'de> de::VariantAccess<'de> for &mut Decoder<'de> {
    type Error = PceError;
    fn unit_variant(self) -> Result<(), PceError> {
        Ok(())
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, PceError> {
        self.nested(|d| seed.deserialize(d))
    }
    fn tuple_variant<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, PceError> {
        self.nested(|d| v.visit_seq(Items { de: d, left: len }))
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, PceError> {
        self.nested(|d| {
            v.visit_seq(Items {
                de: d,
                left: fields.len(),
            })
        })
    }
}

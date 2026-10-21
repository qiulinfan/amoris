//! Only the render feed uses compressed blocks. The checkpoint's own Serde representation,
//! persistence schema and JSON remain the exact raw words (water.md 39).
use std::{fmt, sync::Arc};

use lz4_flex::block::{compress_into, decompress_into, get_maximum_output_size};
use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
    ser::SerializeTuple,
};

use crate::water_field::{CheckedCheckpoint, FieldCheckpoint};

pub(super) fn serialize<S: serde::Serializer>(
    checkpoint: &Option<Arc<CheckedCheckpoint>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if serializer.is_human_readable() {
        return checkpoint.serialize(serializer);
    }
    match checkpoint {
        Some(checkpoint) => serializer.serialize_some(&Packed(checkpoint)),
        None => serializer.serialize_none(),
    }
}

pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Arc<CheckedCheckpoint>>, D::Error> {
    if deserializer.is_human_readable() {
        return Option::<Arc<CheckedCheckpoint>>::deserialize(deserializer);
    }
    Option::<Unpacked>::deserialize(deserializer).map(|c| c.map(|c| Arc::new(c.0)))
}

struct Packed<'a>(&'a CheckedCheckpoint);
impl Serialize for Packed<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let c = self.0;
        let mut input = Vec::with_capacity(c.columns.len() * 16 + c.far_faces.len() * 4);
        #[cfg(target_endian = "little")]
        {
            input.extend_from_slice(bytemuck::cast_slice(&c.columns));
            input.extend_from_slice(bytemuck::cast_slice(&c.far_faces));
        }
        #[cfg(target_endian = "big")]
        for word in c.columns.iter().flatten().chain(&c.far_faces) {
            input.extend_from_slice(&word.to_le_bytes());
        }
        let mut compressed = vec![0; get_maximum_output_size(input.len())];
        let written = compress_into(&input, &mut compressed).map_err(serde::ser::Error::custom)?;
        compressed.truncate(written);
        let mut tuple = serializer.serialize_tuple(3)?;
        tuple.serialize_element(&c.size)?;
        tuple.serialize_element(&c.datum)?;
        struct Bytes<'a>(&'a [u8]);
        impl Serialize for Bytes<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(self.0)
            }
        }
        tuple.serialize_element(&Bytes(&compressed))?;
        tuple.end()
    }
}

#[derive(Debug)]
struct Unpacked(CheckedCheckpoint);
impl<'de> Deserialize<'de> for Unpacked {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Packet;
        impl<'de> Visitor<'de> for Packet {
            type Value = Unpacked;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a resident checkpoint header and bounded LZ4 block")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let size: [u32; 2] = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing checkpoint size"))?;
                let datum: f64 = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing checkpoint datum"))?;
                if size.iter().any(|&n| !(2..=512).contains(&n)) || !datum.is_finite() {
                    return Err(de::Error::custom("invalid checkpoint dimensions or datum"));
                }
                seq.next_element_seed(Payload { size, datum })?
                    .map(Unpacked)
                    .ok_or_else(|| de::Error::custom("missing checkpoint block"))
            }
        }
        deserializer.deserialize_tuple(3, Packet)
    }
}

/// Dimensions have already been bounded before asking the byte deserializer for the payload.
/// bincode's slice decoder visits borrowed bytes, so a hostile length cannot allocate a buffer.
struct Payload {
    size: [u32; 2],
    datum: f64,
}
impl<'de> DeserializeSeed<'de> for Payload {
    type Value = CheckedCheckpoint;
    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_bytes(self)
    }
}
impl<'de> Visitor<'de> for Payload {
    type Value = CheckedCheckpoint;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a bounded compressed checkpoint byte block")
    }
    fn visit_bytes<E: de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
        // Both dimensions are in 2..=512, including on 32-bit targets. The header determines
        // the exact output size; no attacker-controlled compressed-size prefix is used.
        let columns = self.size[0] as usize * self.size[1] as usize;
        let faces = (self.size[0] + self.size[1]) as usize;
        let length = columns * 16 + faces * 4;
        if bytes.len() > get_maximum_output_size(length) {
            return Err(E::custom("checkpoint compressed block exceeds its bound"));
        }
        // Decode directly into aligned typed storage. The small tail temporarily occupies
        // additional columns; copying only the faces avoids a second full-field allocation.
        let mut data = vec![[0u32; 4]; columns + faces.div_ceil(4)];
        let output: &mut [u8] = bytemuck::cast_slice_mut(&mut data);
        let written = decompress_into(bytes, &mut output[..length]).map_err(E::custom)?;
        if written != length {
            return Err(E::custom(
                "checkpoint decompressed length does not match its header",
            ));
        }
        #[cfg(target_endian = "big")]
        for word in data.iter_mut().flatten() {
            *word = u32::from_le(*word);
        }
        let far_faces = bytemuck::cast_slice::<[u32; 4], u32>(&data[columns..])[..faces].to_vec();
        data.truncate(columns);
        FieldCheckpoint {
            size: self.size,
            datum: self.datum,
            columns: data,
            far_faces,
        }
        .checked()
        .map_err(E::custom)
    }
    fn visit_byte_buf<E: de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
        self.visit_bytes(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Envelope(#[serde(with = "super")] Option<Arc<CheckedCheckpoint>>);

    struct Wire([u32; 2], f64, Vec<u8>);
    impl Serialize for Wire {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            let mut tuple = s.serialize_tuple(3)?;
            tuple.serialize_element(&self.0)?;
            tuple.serialize_element(&self.1)?;
            struct Bytes<'a>(&'a [u8]);
            impl Serialize for Bytes<'_> {
                fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                    s.serialize_bytes(self.0)
                }
            }
            tuple.serialize_element(&Bytes(&self.2))?;
            tuple.end()
        }
    }
    fn compress(input: &[u8]) -> Vec<u8> {
        let mut out = vec![0; get_maximum_output_size(input.len())];
        let n = compress_into(input, &mut out).unwrap();
        out.truncate(n);
        out
    }
    fn decode(w: &Wire) -> Result<Unpacked, bincode2::error::DecodeError> {
        let config = bincode2::config::standard();
        let bytes = bincode2::serde::encode_to_vec(w, config).unwrap();
        bincode2::serde::decode_from_slice(&bytes, config).map(|(c, used)| {
            assert_eq!(used, bytes.len());
            c
        })
    }
    fn raw(size: [u32; 2]) -> FieldCheckpoint {
        let mut c = FieldCheckpoint::new(
            size,
            -0.0,
            (0..size[0] * size[1])
                .map(|i| [(-0.0f32).to_bits(), i + 1, (-f32::MAX).to_bits(), i])
                .collect(),
        );
        for (i, face) in c.far_faces.iter_mut().enumerate() {
            *face = if i % 2 == 0 {
                (-0.0f32).to_bits()
            } else {
                i as u32
            };
        }
        c
    }

    #[test]
    fn roundtrip_exact_words_identities_and_json_without_changing_raw_serde() {
        for size in [[2, 2], [2, 3], [3, 5], [512, 512]] {
            let raw = raw(size);
            let checked = Arc::new(raw.clone().checked().unwrap());
            let envelope = Envelope(Some(checked.clone()));
            let json = serde_json::to_value(&envelope).unwrap();
            assert_eq!(json, serde_json::to_value(Some(&raw)).unwrap());
            let from_json: Envelope = serde_json::from_value(json).unwrap();
            let config = bincode2::config::standard();
            let bytes = bincode2::serde::encode_to_vec(&envelope, config).unwrap();
            let (decoded, used): (Envelope, _) =
                bincode2::serde::decode_from_slice(&bytes, config).unwrap();
            assert_eq!(used, bytes.len());
            for restored in [decoded.0.unwrap(), from_json.0.unwrap()] {
                assert!(!Arc::ptr_eq(&restored, &checked));
                assert_eq!(restored.columns, raw.columns);
                assert_eq!(restored.far_faces, raw.far_faces);
                assert_eq!(restored.datum.to_bits(), raw.datum.to_bits());
                assert_eq!(restored.identity(), checked.identity());
            }
            assert_eq!(
                bincode2::serde::encode_to_vec(&raw, config).unwrap(),
                bincode2::serde::encode_to_vec(&checked, config).unwrap()
            );
        }
        let none = Envelope(None);
        let bytes = bincode2::serde::encode_to_vec(&none, bincode2::config::standard()).unwrap();
        let (decoded, _): (Envelope, _) =
            bincode2::serde::decode_from_slice(&bytes, bincode2::config::standard()).unwrap();
        assert!(decoded.0.is_none());
    }

    #[test]
    fn invalid_headers_blocks_lengths_and_new_field_values_are_rejected() {
        let c = raw([3, 5]);
        let mut input = Vec::new();
        for word in c.columns.iter().flatten().chain(&c.far_faces) {
            input.extend_from_slice(&word.to_le_bytes());
        }
        assert_eq!(
            decode(&Wire(c.size, c.datum, compress(&input)))
                .unwrap()
                .0
                .columns,
            c.columns
        );
        for size in [[0, 5], [1, 5], [513, 5], [u32::MAX, u32::MAX]] {
            assert!(decode(&Wire(size, 0.0, vec![])).is_err());
        }
        for datum in [f64::NAN, f64::INFINITY] {
            assert!(decode(&Wire(c.size, datum, vec![])).is_err());
        }
        // The borrowed slice decoder rejects a huge claimed length before allocating or
        // decompressing any payload. These bytes contain only a header and length prefix.
        let config = bincode2::config::standard();
        let mut short = bincode2::serde::encode_to_vec((c.size, c.datum), config).unwrap();
        short.push(253); // bincode standard's u64 varint marker
        short.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(bincode2::serde::decode_from_slice::<Unpacked, _>(&short, config).is_err());
        let block = compress(&input);
        for bad in [
            vec![],
            vec![0, 0, 0],
            block[..block.len() - 1].to_vec(),
            compress(&input[..input.len() - 4]),
            compress(&[input.clone(), vec![0; 4]].concat()),
            vec![0; get_maximum_output_size(input.len()) + 1],
        ] {
            assert!(decode(&Wire(c.size, c.datum, bad)).is_err());
        }
        for (offset, word) in [
            (4, f32::NAN.to_bits()),
            (4, (-1.0f32).to_bits()),
            (input.len() - 4, f32::INFINITY.to_bits()),
        ] {
            let mut bad = input.clone();
            bad[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
            assert!(decode(&Wire(c.size, c.datum, compress(&bad))).is_err());
        }
    }
}

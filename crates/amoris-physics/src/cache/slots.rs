//! The shape slots of the cache's layout 3 (docs/spec/persistence.md 8): Rapier's `PhysicsWorld`
//! through its own `serde` output, except that each collider's `shape` field is written as a slot,
//! `ShapeSlot::Inline(shape)` (bincode: the variant index 0, then the shape) or
//! `ShapeSlot::ByRef(spec, key)` (the index 1, then the body's 64-bit spec hash and the 32-byte key
//! of its components, `crate::shapes`), whose shape is the one the body's components build. A block
//! volume chunk's collider (`crate::volumes`) has `ShapeSlot::Chunk(record)` (the index 2, then its
//! `ChunkRecord`: the chunk's key, the tick it was checked at, the volume's form and the shape's
//! key), whose shape is the one the chunk's content with that key builds, or, while its chunk has
//! changed since the solver last looked at it, `ShapeSlot::ChunkInline(record, shape)` (the index
//! 3). Layout 2 had the first two.
//!
//! Rapier keeps a collider's fields private and derives its `serde`, so the slots are written and
//! read by wrapping the serializer and the deserializer, in layers that keep the wrapped code to
//! the colliders' containers (a wrapper's code is compiled again for every type it passes
//! through):
//!
//! - [`WorldOut`] and [`WorldIn`] write and read the `PhysicsWorld` field by field as its derived
//!   implementation does (the fields in declaration order, the skipped workspaces left out), every
//!   field straight through bincode except `colliders`;
//! - the outer layer follows the `ColliderSet`, its arena and the arena's entries, counting the
//!   colliders in the order Rapier writes them (the order of `ColliderSet::iter`);
//! - an entry's fields (an `Entry::Occupied`'s generation and value) go through the inner layer,
//!   which passes everything straight through except the struct `Collider`, whose field `shape` it
//!   writes or reads as a slot.
//!
//! Nothing below a collider, a shape least of all, is wrapped. bincode reads a struct as a
//! sequence of its fields, which is how the slots are found on reading.
//!
//! A by-reference slot is read as a shape built elsewhere: Rapier's derived `Deserialize` asks the
//! wrapper for a value of its field's type, generic there, and [`cast`] hands it the
//! `SharedShape` after `typeid` has shown that the type is `SharedShape`.

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use rapier3d::prelude::{ColliderSet, PhysicsWorld, SharedShape};
use serde::de::{
    self, Deserialize, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde::ser::{
    Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};

use crate::shapes::{self, ShapeKey};
use crate::volumes::ChunkRecord;

const SLOT: &str = "ShapeSlot";
const VARIANTS: &[&str] = &["Inline", "ByRef", "Chunk", "ChunkInline"];

/// The fields of `PhysicsWorld` its derived `serde` writes, in order (rapier3d 0.36.0,
/// `src/pipeline/physics_world.rs`; `physics_pipeline`, `collision_pipeline` and `ccd_solver` are
/// skipped).
const WORLD_FIELDS: &[&str] = &[
    "gravity",
    "integration_parameters",
    "islands",
    "broad_phase",
    "narrow_phase",
    "bodies",
    "colliders",
    "impulse_joints",
    "multibody_joints",
    "soft_bodies",
];

/// A by-reference slot: the body's spec hash and its components' key.
pub(super) type ByRef = (u64, ShapeKey);

/// How a collider's shape is written.
#[derive(Clone, Copy, Debug)]
pub(super) enum Out {
    Inline,
    ByRef(ByRef),
    Chunk(ChunkRecord),
    ChunkInline(ChunkRecord),
}

/// How a collider's shape was read: inline, by reference, or a volume chunk's (by reference or
/// not).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum In {
    Inline,
    ByRef(ByRef),
    Chunk(ChunkRecord),
}

// --- Writing -------------------------------------------------------------------------------------

/// How each collider's shape is written, in Rapier's order.
pub(super) struct Writing<'a> {
    slots: &'a [Out],
    next: Cell<usize>,
    /// Writes each shape as Rapier does, with no slot (for the test that the wrappers write
    /// Rapier's own bytes).
    transparent: bool,
}

impl<'a> Writing<'a> {
    pub(super) fn new(slots: &'a [Out]) -> Writing<'a> {
        Writing {
            slots,
            next: Cell::new(0),
            transparent: false,
        }
    }

    #[cfg(test)]
    pub(super) fn transparent() -> Writing<'a> {
        Writing {
            slots: &[],
            next: Cell::new(0),
            transparent: true,
        }
    }

    /// How many shape slots were written.
    pub(super) fn written(&self) -> usize {
        self.next.get()
    }
}

/// A `PhysicsWorld` written with its colliders' shapes in slots.
pub(super) struct WorldOut<'a> {
    pub(super) world: &'a PhysicsWorld,
    pub(super) writing: &'a Writing<'a>,
}

impl Serialize for WorldOut<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let w = self.world;
        let mut f = s.serialize_struct("PhysicsWorld", WORLD_FIELDS.len())?;
        f.serialize_field("gravity", &w.gravity)?;
        f.serialize_field("integration_parameters", &w.integration_parameters)?;
        f.serialize_field("islands", &w.islands)?;
        f.serialize_field("broad_phase", &w.broad_phase)?;
        f.serialize_field("narrow_phase", &w.narrow_phase)?;
        f.serialize_field("bodies", &w.bodies)?;
        let colliders = Outer {
            value: &w.colliders,
            writing: self.writing,
        };
        f.serialize_field("colliders", &colliders)?;
        f.serialize_field("impulse_joints", &w.impulse_joints)?;
        f.serialize_field("multibody_joints", &w.multibody_joints)?;
        f.serialize_field("soft_bodies", &w.soft_bodies)?;
        f.end()
    }
}

/// `value` written through the outer layer.
struct Outer<'v, 'w, T: ?Sized> {
    value: &'v T,
    writing: &'w Writing<'w>,
}

impl<T: ?Sized + Serialize> Serialize for Outer<'_, '_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(OSer { s, w: self.writing })
    }
}

/// `value` written through the inner layer.
struct Inner<'v, 'w, T: ?Sized> {
    value: &'v T,
    writing: &'w Writing<'w>,
}

impl<T: ?Sized + Serialize> Serialize for Inner<'_, '_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(ISer { s, w: self.writing })
    }
}

enum Slot<'v, T: ?Sized> {
    Inline(&'v T),
    ByRef(ByRef),
    Chunk(ChunkRecord),
    ChunkInline(ChunkRecord, &'v T),
}

impl<T: ?Sized + Serialize> Serialize for Slot<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Slot::Inline(shape) => s.serialize_newtype_variant(SLOT, 0, VARIANTS[0], *shape),
            Slot::ByRef(r) => s.serialize_newtype_variant(SLOT, 1, VARIANTS[1], r),
            Slot::Chunk(r) => s.serialize_newtype_variant(SLOT, 2, VARIANTS[2], r),
            Slot::ChunkInline(r, shape) => {
                s.serialize_newtype_variant(SLOT, 3, VARIANTS[3], &(r, *shape))
            }
        }
    }
}

/// The outer layer's serializer: every value below is written through the outer layer, except
/// the fields of a struct variant (an arena entry's), which go through the inner layer.
struct OSer<'w, S> {
    s: S,
    w: &'w Writing<'w>,
}

impl<'w, S> OSer<'w, S> {
    fn wrap<'v, T: ?Sized>(&self, value: &'v T) -> Outer<'v, 'w, T> {
        Outer {
            value,
            writing: self.w,
        }
    }
}

fn oser<'w, N>(w: &'w Writing<'w>, s: N) -> OSer<'w, N> {
    OSer { s, w }
}

macro_rules! forward_ser {
    ($($f:ident($t:ty)),* $(,)?) => {
        $(fn $f(self, v: $t) -> Result<S::Ok, S::Error> {
            self.s.$f(v)
        })*
    };
}

impl<'w, S: Serializer> Serializer for OSer<'w, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = OSer<'w, S::SerializeSeq>;
    type SerializeTuple = OSer<'w, S::SerializeTuple>;
    type SerializeTupleStruct = OSer<'w, S::SerializeTupleStruct>;
    type SerializeTupleVariant = OSer<'w, S::SerializeTupleVariant>;
    type SerializeMap = OSer<'w, S::SerializeMap>;
    type SerializeStruct = OSer<'w, S::SerializeStruct>;
    type SerializeStructVariant = OSer<'w, S::SerializeStructVariant>;

    forward_ser! {
        serialize_bool(bool), serialize_i8(i8), serialize_i16(i16), serialize_i32(i32),
        serialize_i64(i64), serialize_i128(i128), serialize_u8(u8), serialize_u16(u16),
        serialize_u32(u32), serialize_u64(u64), serialize_u128(u128), serialize_f32(f32),
        serialize_f64(f64), serialize_char(char), serialize_str(&str), serialize_bytes(&[u8]),
        serialize_unit_struct(&'static str),
    }

    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.s.serialize_none()
    }

    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<S::Ok, S::Error> {
        let v = self.wrap(v);
        self.s.serialize_some(&v)
    }

    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.s.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.s.serialize_unit_variant(name, index, variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        let v = self.wrap(v);
        self.s.serialize_newtype_struct(name, &v)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        let v = self.wrap(v);
        self.s.serialize_newtype_variant(name, index, variant, &v)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        Ok(oser(self.w, self.s.serialize_seq(len)?))
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        Ok(oser(self.w, self.s.serialize_tuple(len)?))
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        Ok(oser(self.w, self.s.serialize_tuple_struct(name, len)?))
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        Ok(oser(
            self.w,
            self.s.serialize_tuple_variant(name, index, variant, len)?,
        ))
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        Ok(oser(self.w, self.s.serialize_map(len)?))
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        Ok(oser(self.w, self.s.serialize_struct(name, len)?))
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        Ok(oser(
            self.w,
            self.s.serialize_struct_variant(name, index, variant, len)?,
        ))
    }

    fn is_human_readable(&self) -> bool {
        self.s.is_human_readable()
    }
}

impl<S: SerializeSeq> SerializeSeq for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_element(&v)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

impl<S: SerializeTuple> SerializeTuple for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_element(&v)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

impl<S: SerializeTupleStruct> SerializeTupleStruct for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_field(&v)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

impl<S: SerializeTupleVariant> SerializeTupleVariant for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_field(&v)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

impl<S: SerializeMap> SerializeMap for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, k: &T) -> Result<(), S::Error> {
        let k = self.wrap(k);
        self.s.serialize_key(&k)
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_value(&v)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

impl<S: SerializeStruct> SerializeStruct for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), S::Error> {
        let v = self.wrap(v);
        self.s.serialize_field(key, &v)
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.s.skip_field(key)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

/// An arena entry's fields: through the inner layer.
impl<S: SerializeStructVariant> SerializeStructVariant for OSer<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), S::Error> {
        let v = Inner {
            value: v,
            writing: self.w,
        };
        self.s.serialize_field(key, &v)
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.s.skip_field(key)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

/// The inner layer's serializer: every value straight through, except a struct, whose fields
/// [`IFields`] writes (a collider's shape in its slot).
struct ISer<'w, S> {
    s: S,
    w: &'w Writing<'w>,
}

impl<'w, S: Serializer> Serializer for ISer<'w, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = IFields<'w, S::SerializeStruct>;
    type SerializeStructVariant = S::SerializeStructVariant;

    forward_ser! {
        serialize_bool(bool), serialize_i8(i8), serialize_i16(i16), serialize_i32(i32),
        serialize_i64(i64), serialize_i128(i128), serialize_u8(u8), serialize_u16(u16),
        serialize_u32(u32), serialize_u64(u64), serialize_u128(u128), serialize_f32(f32),
        serialize_f64(f64), serialize_char(char), serialize_str(&str), serialize_bytes(&[u8]),
        serialize_unit_struct(&'static str),
    }

    fn serialize_none(self) -> Result<S::Ok, S::Error> {
        self.s.serialize_none()
    }

    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<S::Ok, S::Error> {
        self.s.serialize_some(v)
    }

    fn serialize_unit(self) -> Result<S::Ok, S::Error> {
        self.s.serialize_unit()
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
    ) -> Result<S::Ok, S::Error> {
        self.s.serialize_unit_variant(name, index, variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        self.s.serialize_newtype_struct(name, v)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        self.s.serialize_newtype_variant(name, index, variant, v)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.s.serialize_seq(len)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.s.serialize_tuple(len)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.s.serialize_tuple_struct(name, len)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.s.serialize_tuple_variant(name, index, variant, len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.s.serialize_map(len)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, S::Error> {
        Ok(IFields {
            s: self.s.serialize_struct(name, len)?,
            w: self.w,
            collider: name == "Collider",
        })
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.s.serialize_struct_variant(name, index, variant, len)
    }

    fn is_human_readable(&self) -> bool {
        self.s.is_human_readable()
    }
}

/// A struct's fields below an arena entry: a collider's shape in its slot, everything else
/// straight through.
struct IFields<'w, S> {
    s: S,
    w: &'w Writing<'w>,
    collider: bool,
}

impl<S: SerializeStruct> SerializeStruct for IFields<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), S::Error> {
        if !(self.collider && key == "shape") {
            return self.s.serialize_field(key, v);
        }
        let k = self.w.next.get();
        self.w.next.set(k + 1);
        if self.w.transparent {
            return self.s.serialize_field(key, v);
        }
        match self.w.slots.get(k).copied().unwrap_or(Out::Inline) {
            Out::Inline => self.s.serialize_field(key, &Slot::Inline(v)),
            Out::ByRef(r) => self.s.serialize_field(key, &Slot::<T>::ByRef(r)),
            Out::Chunk(r) => self.s.serialize_field(key, &Slot::<T>::Chunk(r)),
            Out::ChunkInline(r) => self.s.serialize_field(key, &Slot::ChunkInline(r, v)),
        }
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.s.skip_field(key)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.s.end()
    }
}

// --- Reading -------------------------------------------------------------------------------------

/// Where by-reference slots (a fixed body's or a volume chunk's) get their shapes: those `provided`
/// by key, then the table of built shapes; a slot with neither gets a placeholder and is listed in
/// `missing`. Every slot read is listed in `slots`, in Rapier's order.
pub(super) struct Reading<'a> {
    provided: &'a BTreeMap<ShapeKey, SharedShape>,
    pub(super) slots: RefCell<Vec<In>>,
    pub(super) missing: RefCell<Vec<usize>>,
}

impl<'a> Reading<'a> {
    pub(super) fn new(provided: &'a BTreeMap<ShapeKey, SharedShape>) -> Reading<'a> {
        Reading {
            provided,
            slots: RefCell::new(Vec::new()),
            missing: RefCell::new(Vec::new()),
        }
    }

    fn shape(&self, (spec, key): ByRef) -> Result<SharedShape, &'static str> {
        let ordinal = self.slots.borrow().len();
        self.slots.borrow_mut().push(In::ByRef((spec, key)));
        if let Some(s) = self.provided.get(&key) {
            return Ok(s.clone());
        }
        if let Some((s, built_from)) = shapes::recall(&key) {
            return if built_from == spec {
                Ok(s)
            } else {
                Err("a shape slot's key names components of another spec hash")
            };
        }
        self.missing.borrow_mut().push(ordinal);
        Ok(SharedShape::ball(0.0))
    }

    /// A volume chunk's shape by reference: by its key alone, which names its content.
    fn chunk(&self, r: ChunkRecord) -> SharedShape {
        let ordinal = self.slots.borrow().len();
        self.slots.borrow_mut().push(In::Chunk(r));
        if let Some(s) = self.provided.get(&r.key) {
            return s.clone();
        }
        if let Some((s, _)) = shapes::recall(&r.key) {
            return s;
        }
        self.missing.borrow_mut().push(ordinal);
        SharedShape::ball(0.0)
    }
}

/// Moves `shape` out as a `T` when `T` is `SharedShape`.
fn cast<T>(shape: SharedShape) -> Option<T> {
    if typeid::of::<T>() != TypeId::of::<SharedShape>() {
        return None;
    }
    let shape = ManuallyDrop::new(shape);
    // SAFETY: `T` has the TypeId of `SharedShape`, which is 'static and has no lifetime parameter,
    // so `T` is `SharedShape` (typeid's documented guarantee for a 'static type). The value is
    // moved once: `ManuallyDrop` keeps it from being dropped here.
    Some(unsafe { std::mem::transmute_copy::<SharedShape, T>(&shape) })
}

/// A `PhysicsWorld` read with its colliders' shapes from their slots.
pub(super) struct WorldIn<'r> {
    pub(super) r: &'r Reading<'r>,
}

impl<'de> DeserializeSeed<'de> for WorldIn<'_> {
    type Value = PhysicsWorld;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<PhysicsWorld, D::Error> {
        d.deserialize_struct("PhysicsWorld", WORLD_FIELDS, self)
    }
}

impl<'de> Visitor<'de> for WorldIn<'_> {
    type Value = PhysicsWorld;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a PhysicsWorld")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<PhysicsWorld, A::Error> {
        fn field<'de, A: SeqAccess<'de>, T: Deserialize<'de>>(
            a: &mut A,
            i: usize,
        ) -> Result<T, A::Error> {
            a.next_element()?
                .ok_or_else(|| de::Error::invalid_length(i, &"the fields of a PhysicsWorld"))
        }
        let gravity = field(&mut a, 0)?;
        let integration_parameters = field(&mut a, 1)?;
        let islands = field(&mut a, 2)?;
        let broad_phase = field(&mut a, 3)?;
        let narrow_phase = field(&mut a, 4)?;
        let bodies = field(&mut a, 5)?;
        let colliders = a
            .next_element_seed(OSeed::<ColliderSet> {
                r: self.r,
                t: PhantomData,
            })?
            .ok_or_else(|| de::Error::invalid_length(6, &"the fields of a PhysicsWorld"))?;
        let impulse_joints = field(&mut a, 7)?;
        let multibody_joints = field(&mut a, 8)?;
        let soft_bodies = field(&mut a, 9)?;
        Ok(PhysicsWorld {
            gravity,
            integration_parameters,
            physics_pipeline: Default::default(),
            collision_pipeline: Default::default(),
            islands,
            broad_phase,
            narrow_phase,
            bodies,
            colliders,
            impulse_joints,
            multibody_joints,
            soft_bodies,
            ccd_solver: Default::default(),
        })
    }
}

/// A `T` read through the outer layer.
struct OSeed<'r, T> {
    r: &'r Reading<'r>,
    t: PhantomData<T>,
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for OSeed<'_, T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        T::deserialize(ODe { d, r: self.r })
    }
}

/// The outer layer's deserializer: every value below is read through the outer layer, except the
/// fields of a struct variant (an arena entry's), which go through the inner layer.
struct ODe<'r, D> {
    d: D,
    r: &'r Reading<'r>,
}

struct OVis<'r, V> {
    v: V,
    r: &'r Reading<'r>,
}

macro_rules! forward_de {
    ($($f:ident),* $(,)?) => {
        $(fn $f<V: Visitor<'de>>(self, v: V) -> Result<V::Value, D::Error> {
            self.d.$f(OVis { v, r: self.r })
        })*
    };
}

impl<'de, 'r, D: Deserializer<'de>> Deserializer<'de> for ODe<'r, D> {
    type Error = D::Error;

    forward_de! {
        deserialize_any, deserialize_bool, deserialize_i8, deserialize_i16, deserialize_i32,
        deserialize_i64, deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32,
        deserialize_u64, deserialize_u128, deserialize_f32, deserialize_f64, deserialize_char,
        deserialize_str, deserialize_string, deserialize_bytes, deserialize_byte_buf,
        deserialize_option, deserialize_unit, deserialize_seq, deserialize_map,
        deserialize_identifier, deserialize_ignored_any,
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d.deserialize_unit_struct(name, OVis { v, r: self.r })
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d
            .deserialize_newtype_struct(name, OVis { v, r: self.r })
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, D::Error> {
        self.d.deserialize_tuple(len, OVis { v, r: self.r })
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d
            .deserialize_tuple_struct(name, len, OVis { v, r: self.r })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d
            .deserialize_struct(name, fields, OVis { v, r: self.r })
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d
            .deserialize_enum(name, variants, OVis { v, r: self.r })
    }

    fn is_human_readable(&self) -> bool {
        self.d.is_human_readable()
    }
}

macro_rules! forward_visit {
    ($($f:ident($t:ty)),* $(,)?) => {
        $(fn $f<E: de::Error>(self, x: $t) -> Result<Self::Value, E> {
            self.v.$f(x)
        })*
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for OVis<'_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.v.expecting(f)
    }

    forward_visit! {
        visit_bool(bool), visit_i8(i8), visit_i16(i16), visit_i32(i32), visit_i64(i64),
        visit_i128(i128), visit_u8(u8), visit_u16(u16), visit_u32(u32), visit_u64(u64),
        visit_u128(u128), visit_f32(f32), visit_f64(f64), visit_char(char), visit_str(&str),
        visit_borrowed_str(&'de str), visit_string(String), visit_bytes(&[u8]),
        visit_borrowed_bytes(&'de [u8]), visit_byte_buf(Vec<u8>),
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.v.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.v.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.v.visit_some(ODe { d, r: self.r })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.v.visit_newtype_struct(ODe { d, r: self.r })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.v.visit_seq(OSeq { a, r: self.r })
    }

    fn visit_map<A: MapAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.v.visit_map(OMap { a, r: self.r })
    }

    fn visit_enum<A: EnumAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.v.visit_enum(OEnum { a, r: self.r })
    }
}

/// A seed whose value is read through the outer layer.
struct OWrapped<'r, T> {
    seed: T,
    r: &'r Reading<'r>,
}

impl<'de, T: DeserializeSeed<'de>> DeserializeSeed<'de> for OWrapped<'_, T> {
    type Value = T::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T::Value, D::Error> {
        self.seed.deserialize(ODe { d, r: self.r })
    }
}

struct OSeq<'r, A> {
    a: A,
    r: &'r Reading<'r>,
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for OSeq<'_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        self.a.next_element_seed(OWrapped { seed, r: self.r })
    }

    fn size_hint(&self) -> Option<usize> {
        self.a.size_hint()
    }
}

struct OMap<'r, A> {
    a: A,
    r: &'r Reading<'r>,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for OMap<'_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.a.next_key_seed(OWrapped { seed, r: self.r })
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        self.a.next_value_seed(OWrapped { seed, r: self.r })
    }

    fn size_hint(&self) -> Option<usize> {
        self.a.size_hint()
    }
}

struct OEnum<'r, A> {
    a: A,
    r: &'r Reading<'r>,
}

impl<'de, 'r, A: EnumAccess<'de>> EnumAccess<'de> for OEnum<'r, A> {
    type Error = A::Error;
    type Variant = OVariant<'r, A::Variant>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), A::Error> {
        let (value, a) = self.a.variant_seed(seed)?;
        Ok((value, OVariant { a, r: self.r }))
    }
}

struct OVariant<'r, A> {
    a: A,
    r: &'r Reading<'r>,
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for OVariant<'_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.a.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.a.newtype_variant_seed(OWrapped { seed, r: self.r })
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, A::Error> {
        self.a.tuple_variant(len, OVis { v, r: self.r })
    }

    /// An arena entry's fields: through the inner layer.
    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, A::Error> {
        self.a.struct_variant(fields, IVis { v, r: self.r })
    }
}

/// An arena entry's fields, each read through the inner layer.
struct IVis<'r, V> {
    v: V,
    r: &'r Reading<'r>,
}

impl<'de, V: Visitor<'de>> Visitor<'de> for IVis<'_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.v.expecting(f)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.v.visit_seq(ISeq { a, r: self.r })
    }
}

struct ISeq<'r, A> {
    a: A,
    r: &'r Reading<'r>,
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for ISeq<'_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        self.a.next_element_seed(IWrapped { seed, r: self.r })
    }

    fn size_hint(&self) -> Option<usize> {
        self.a.size_hint()
    }
}

/// A seed whose value is read through the inner layer.
struct IWrapped<'r, T> {
    seed: T,
    r: &'r Reading<'r>,
}

impl<'de, T: DeserializeSeed<'de>> DeserializeSeed<'de> for IWrapped<'_, T> {
    type Value = T::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T::Value, D::Error> {
        self.seed.deserialize(IDe { d, r: self.r })
    }
}

/// The inner layer's deserializer: every value straight through, except a struct, whose fields
/// [`IStructVis`] reads (a collider's shape from its slot).
struct IDe<'r, D> {
    d: D,
    r: &'r Reading<'r>,
}

macro_rules! plain_de {
    ($($f:ident),* $(,)?) => {
        $(fn $f<V: Visitor<'de>>(self, v: V) -> Result<V::Value, D::Error> {
            self.d.$f(v)
        })*
    };
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for IDe<'_, D> {
    type Error = D::Error;

    plain_de! {
        deserialize_any, deserialize_bool, deserialize_i8, deserialize_i16, deserialize_i32,
        deserialize_i64, deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32,
        deserialize_u64, deserialize_u128, deserialize_f32, deserialize_f64, deserialize_char,
        deserialize_str, deserialize_string, deserialize_bytes, deserialize_byte_buf,
        deserialize_option, deserialize_unit, deserialize_seq, deserialize_map,
        deserialize_identifier, deserialize_ignored_any,
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d.deserialize_unit_struct(name, v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d.deserialize_newtype_struct(name, v)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, D::Error> {
        self.d.deserialize_tuple(len, v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d.deserialize_tuple_struct(name, len, v)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, D::Error> {
        let shape = (name == "Collider")
            .then(|| fields.iter().position(|f| *f == "shape"))
            .flatten();
        let v = IStructVis {
            v,
            r: self.r,
            shape,
        };
        self.d.deserialize_struct(name, fields, v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, D::Error> {
        self.d.deserialize_enum(name, variants, v)
    }

    fn is_human_readable(&self) -> bool {
        self.d.is_human_readable()
    }
}

/// A struct's fields below an arena entry, read in order: a collider's shape (the field at
/// `shape`) from its slot, everything else straight through.
struct IStructVis<'r, V> {
    v: V,
    r: &'r Reading<'r>,
    shape: Option<usize>,
}

impl<'de, V: Visitor<'de>> Visitor<'de> for IStructVis<'_, V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.v.expecting(f)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.v.visit_seq(IStructSeq {
            a,
            r: self.r,
            shape: self.shape,
            i: 0,
        })
    }
}

struct IStructSeq<'r, A> {
    a: A,
    r: &'r Reading<'r>,
    shape: Option<usize>,
    i: usize,
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for IStructSeq<'_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        let i = self.i;
        self.i += 1;
        if self.shape == Some(i) {
            self.a.next_element_seed(SlotSeed { seed, r: self.r })
        } else {
            self.a.next_element_seed(seed)
        }
    }

    fn size_hint(&self) -> Option<usize> {
        self.a.size_hint()
    }
}

/// A collider's `shape` field: its slot.
struct SlotSeed<'r, T> {
    seed: T,
    r: &'r Reading<'r>,
}

impl<'de, T: DeserializeSeed<'de>> DeserializeSeed<'de> for SlotSeed<'_, T> {
    type Value = T::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T::Value, D::Error> {
        d.deserialize_enum(SLOT, VARIANTS, self)
    }
}

impl<'de, T: DeserializeSeed<'de>> Visitor<'de> for SlotSeed<'_, T> {
    type Value = T::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a collider's shape slot")
    }

    fn visit_enum<A: EnumAccess<'de>>(self, a: A) -> Result<T::Value, A::Error> {
        let (index, v): (u32, _) = a.variant()?;
        match index {
            0 => {
                self.r.slots.borrow_mut().push(In::Inline);
                v.newtype_variant_seed(self.seed)
            }
            1 => {
                let r: ByRef = v.newtype_variant()?;
                let shape = self.r.shape(r).map_err(de::Error::custom)?;
                cast(shape)
                    .ok_or_else(|| de::Error::custom("a collider's shape is not a SharedShape"))
            }
            2 => {
                let r: ChunkRecord = v.newtype_variant()?;
                cast(self.r.chunk(r))
                    .ok_or_else(|| de::Error::custom("a collider's shape is not a SharedShape"))
            }
            3 => v.newtype_variant_seed(ChunkInline {
                seed: self.seed,
                r: self.r,
            }),
            _ => Err(de::Error::custom(
                "a shape slot is 0 (inline), 1 (by reference), 2 (a chunk's by reference) or 3 \
                 (a chunk's inline)",
            )),
        }
    }
}

/// A chunk's inline slot: its record, then the shape.
struct ChunkInline<'r, T> {
    seed: T,
    r: &'r Reading<'r>,
}

impl<'de, T: DeserializeSeed<'de>> DeserializeSeed<'de> for ChunkInline<'_, T> {
    type Value = T::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T::Value, D::Error> {
        d.deserialize_tuple(2, self)
    }
}

impl<'de, T: DeserializeSeed<'de>> Visitor<'de> for ChunkInline<'_, T> {
    type Value = T::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a chunk's record and shape")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<T::Value, A::Error> {
        let r: ChunkRecord = a
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &"a chunk's record and shape"))?;
        self.r.slots.borrow_mut().push(In::Chunk(r));
        a.next_element_seed(self.seed)?
            .ok_or_else(|| de::Error::invalid_length(1, &"a chunk's record and shape"))
    }
}

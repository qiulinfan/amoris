//! The component registry (docs/spec/architecture.md 4.1; script-host.md 7.1): every component a
//! script, a tool or a presenter can name, engine or project, with its stable name, schema
//! version, doc, JSON Schema (through `schemars`, what tools show, charter 3.6) and, for components
//! scripts read and write, its `ComponentSchema` of typed fields and numeric slots. One registry per
//! world, as a resource (its `ComponentId`s belong to that world); it is not world state.

use std::any::TypeId;
use std::sync::Arc;

use bevy_ecs::component::{Component, ComponentId};
use bevy_ecs::prelude::{Resource, World};
use pocket_contract::{Problem, detail, suggest_names};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::entity::{EntityId, MAX_ENTITY_ID};
use crate::persisted::Persisted;
use crate::time::Tick;

/// Where a component is declared.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub enum ComponentOrigin {
    /// A Rust type of the engine.
    Engine,
    /// Declared by the project in TypeScript (script-host.md 7.3).
    Project,
}

/// A field's type as scripts see it (script-host.md 6 and 7.1); vectors and quaternions of f64.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FieldType {
    F64,
    I32,
    U32,
    Tick,
    Bool,
    Entity,
    Str,
    Enum(Arc<[Box<str>]>),
    Vec2,
    Vec3,
    Vec4,
    Quat,
}

impl FieldType {
    /// Numeric slots: one per scalar, bool, enum or entity; two, three or four per vector; none
    /// for a string.
    pub fn slots(&self) -> u16 {
        match self {
            FieldType::Str => 0,
            FieldType::Vec2 => 2,
            FieldType::Vec3 => 3,
            FieldType::Vec4 | FieldType::Quat => 4,
            _ => 1,
        }
    }

    /// The field's JSON Schema.
    pub fn json_schema(&self) -> Value {
        let comps = |names: &[&str]| {
            let props: serde_json::Map<String, Value> = names
                .iter()
                .map(|n| ((*n).to_owned(), json!({"type": "number"})))
                .collect();
            json!({"type": "object", "properties": props, "required": names,
                   "additionalProperties": false})
        };
        match self {
            FieldType::F64 => json!({"type": "number"}),
            FieldType::I32 => json!({"type": "integer", "format": "int32"}),
            FieldType::U32 => json!({"type": "integer", "format": "uint32", "minimum": 0}),
            FieldType::Tick => json!({"type": "integer", "minimum": 0, "maximum": MAX_ENTITY_ID}),
            FieldType::Bool => json!({"type": "boolean"}),
            FieldType::Entity => {
                json!({"type": ["integer", "null"], "minimum": 1, "maximum": MAX_ENTITY_ID})
            }
            FieldType::Str => json!({"type": "string"}),
            FieldType::Enum(v) => {
                json!({"type": "string", "enum": v.iter().map(|s| &**s).collect::<Vec<_>>()})
            }
            FieldType::Vec2 => comps(&["x", "y"]),
            FieldType::Vec3 => comps(&["x", "y", "z"]),
            FieldType::Vec4 | FieldType::Quat => comps(&["x", "y", "z", "w"]),
        }
    }
}

/// A field's default value.
#[derive(Clone, PartialEq, Debug)]
pub enum FieldValue {
    F64(f64),
    I32(i32),
    U32(u32),
    Tick(Tick),
    Bool(bool),
    Entity(Option<EntityId>),
    Str(Box<str>),
    /// A variant index.
    Enum(u32),
    Vec2([f64; 2]),
    Vec3([f64; 3]),
    Vec4([f64; 4]),
    Quat([f64; 4]),
}

/// One field: its name (`^[a-z][a-z0-9_]*$`), type, default, doc and first numeric slot.
#[derive(Clone, PartialEq, Debug)]
pub struct FieldSchema {
    pub name: Box<str>,
    pub ty: FieldType,
    pub default: FieldValue,
    pub doc: Box<str>,
    pub first_slot: u16,
}

/// A component as scripts see it: declaration order is slot order.
#[derive(Clone, PartialEq, Debug)]
pub struct ComponentSchema {
    /// `^[A-Z][A-Za-z0-9]*$`, unique across engine and project.
    pub name: Box<str>,
    pub origin: ComponentOrigin,
    pub version: u32,
    pub doc: Box<str>,
    pub fields: Box<[FieldSchema]>,
}

/// The layout every project component shares (script-host.md 7.3): the numeric slots and the
/// strings in declaration order.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ProjectValues {
    pub nums: Box<[f64]>,
    pub strs: Box<[Arc<str>]>,
}

impl FieldValue {
    /// Why this default does not fit `ty`, if it does not: a variant of another type, a non-finite
    /// number (numeric.md 7: none enters persisted state), a tick past 2^53 - 1, or an enum index
    /// past the variants.
    pub fn misfit(&self, ty: &FieldType) -> Option<String> {
        let finite = |xs: &[f64]| xs.iter().all(|x| x.is_finite());
        let fits = match (self, ty) {
            (FieldValue::F64(x), FieldType::F64) => finite(&[*x]),
            (FieldValue::Vec2(v), FieldType::Vec2) => finite(v),
            (FieldValue::Vec3(v), FieldType::Vec3) => finite(v),
            (FieldValue::Vec4(v), FieldType::Vec4) | (FieldValue::Quat(v), FieldType::Quat) => {
                finite(v)
            }
            (FieldValue::Tick(t), FieldType::Tick) => *t <= Tick::MAX,
            (FieldValue::Enum(i), FieldType::Enum(v)) => {
                usize::try_from(*i).is_ok_and(|i| i < v.len())
            }
            (FieldValue::I32(_), FieldType::I32)
            | (FieldValue::U32(_), FieldType::U32)
            | (FieldValue::Bool(_), FieldType::Bool)
            | (FieldValue::Entity(_), FieldType::Entity)
            | (FieldValue::Str(_), FieldType::Str) => true,
            _ => {
                return Some(format!(
                    "its default {self:?} is not a value of type {ty:?}"
                ));
            }
        };
        (!fits).then(|| format!("its default {self:?} is out of range for type {ty:?}"))
    }
}

/// Why an enum's variants are not a valid list, if they are not: empty, an empty name, or a name
/// given twice.
fn variants_misfit(ty: &FieldType) -> Option<String> {
    let FieldType::Enum(v) = ty else {
        return None;
    };
    if v.is_empty() {
        return Some("its enum has no variants".into());
    }
    if v.iter().any(|n| n.is_empty()) {
        return Some("its enum has an empty variant name".into());
    }
    let mut sorted: Vec<&str> = v.iter().map(|n| &**n).collect();
    sorted.sort_unstable();
    sorted
        .windows(2)
        .find(|w| w[0] == w[1])
        .map(|w| format!("its enum names the variant '{}' twice", w[0]))
}

/// `sim.component_field_invalid {component, field}`, with the reason in the message.
fn field_invalid(component: &str, field: &str, why: &str) -> Problem {
    Problem::new(
        "sim.component_field_invalid",
        format!("Field '{field}' of {component} is invalid: {why}."),
        detail([("component", json!(component)), ("field", json!(field))]),
    )
}

fn invalid_name(what: &str, name: &str, pattern: &str) -> Problem {
    Problem::new(
        "sim.component_name_invalid",
        format!("'{name}' is not a valid {what} name: it must match {pattern}."),
        detail([("name", json!(name)), ("pattern", json!(pattern))]),
    )
}

/// `^[A-Z][A-Za-z0-9]*$`, at most 64 bytes.
pub fn valid_component_name(name: &str) -> bool {
    let mut b = name.bytes();
    name.len() <= 64
        && matches!(b.next(), Some(c) if c.is_ascii_uppercase())
        && b.all(|c| c.is_ascii_alphanumeric())
}

/// `^[a-z][a-z0-9_]*$`, at most 64 bytes.
pub fn valid_field_name(name: &str) -> bool {
    let mut b = name.bytes();
    name.len() <= 64
        && matches!(b.next(), Some(c) if c.is_ascii_lowercase())
        && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

impl ComponentSchema {
    /// A schema from its fields `(name, type, default, doc)`, slots assigned in order. Names are
    /// checked (`sim.component_name_invalid`); a field that is repeated, undocumented (the docs
    /// become the `.d.ts` comments agents read), has a default that does not fit its type (see
    /// [`FieldValue::misfit`]) or an enum without distinct variants, or that takes the component
    /// past 65,535 numeric slots is `sim.component_field_invalid`.
    pub fn new(
        name: &str,
        origin: ComponentOrigin,
        version: u32,
        doc: &str,
        fields: Vec<(&str, FieldType, FieldValue, &str)>,
    ) -> Result<ComponentSchema, Problem> {
        if !valid_component_name(name) {
            return Err(invalid_name("component", name, "^[A-Z][A-Za-z0-9]*$"));
        }
        let mut slot: u16 = 0;
        let mut out = Vec::with_capacity(fields.len());
        for (fname, ty, default, fdoc) in fields {
            if !valid_field_name(fname) {
                return Err(invalid_name("field", fname, "^[a-z][a-z0-9_]*$"));
            }
            if out.iter().any(|f: &FieldSchema| &*f.name == fname) {
                return Err(field_invalid(name, fname, "it is declared twice"));
            }
            if fdoc.trim().is_empty() {
                return Err(field_invalid(name, fname, "it has no doc"));
            }
            if let Some(why) = variants_misfit(&ty).or_else(|| default.misfit(&ty)) {
                return Err(field_invalid(name, fname, &why));
            }
            let Some(next) = slot.checked_add(ty.slots()) else {
                return Err(field_invalid(
                    name,
                    fname,
                    "the component would have more than 65,535 numeric slots",
                ));
            };
            out.push(FieldSchema {
                name: fname.into(),
                ty,
                default,
                doc: fdoc.into(),
                first_slot: slot,
            });
            slot = next;
        }
        Ok(ComponentSchema {
            name: name.into(),
            origin,
            version,
            doc: doc.into(),
            fields: out.into_boxed_slice(),
        })
    }

    /// The number of numeric slots.
    pub fn slots(&self) -> u16 {
        self.fields
            .iter()
            .fold(0u16, |n, f| n.saturating_add(f.ty.slots()))
    }

    /// The JSON Schema of a value of this component: an object of its fields.
    pub fn json_schema(&self) -> Value {
        let props: serde_json::Map<String, Value> = self
            .fields
            .iter()
            .map(|f| {
                let mut s = f.ty.json_schema();
                s["description"] = json!(&*f.doc);
                (f.name.to_string(), s)
            })
            .collect();
        let required: Vec<&str> = self.fields.iter().map(|f| &*f.name).collect();
        json!({"title": &*self.name, "description": &*self.doc, "type": "object",
               "properties": props, "required": required, "additionalProperties": false})
    }
}

/// One registered component.
#[derive(Clone, Debug)]
pub struct ComponentEntry {
    pub name: Box<str>,
    pub origin: ComponentOrigin,
    pub version: u32,
    pub doc: Box<str>,
    /// This world's id of the component.
    pub id: ComponentId,
    /// The Rust type of an engine component.
    pub type_id: Option<TypeId>,
    /// The JSON Schema of a value.
    pub json_schema: Arc<Value>,
    /// Typed fields and slots, for components scripts can name.
    pub script: Option<Arc<ComponentSchema>>,
}

/// What presenters and tools need of an entry: names, versions and JSON Schemas (threads.md 4.1,
/// `RegistryInfo`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ComponentInfo {
    pub name: String,
    pub origin: ComponentOrigin,
    pub version: u32,
    pub doc: String,
    pub schema: Value,
}

/// The registry, a resource of each world; entries ascending by name.
#[derive(Resource, Clone, Debug, Default)]
pub struct ComponentRegistry {
    entries: Vec<ComponentEntry>,
}

impl ComponentRegistry {
    fn add(world: &mut World, entry: ComponentEntry) -> Result<(), Problem> {
        let mut reg = world.get_resource_or_insert_with(ComponentRegistry::default);
        match reg.entries.binary_search_by(|e| e.name.cmp(&entry.name)) {
            Ok(_) => Err(Problem::new(
                "sim.component_duplicate",
                format!("A component named '{}' is already registered.", entry.name),
                detail([("name", json!(&*entry.name))]),
            )),
            Err(i) => {
                reg.entries.insert(i, entry);
                Ok(())
            }
        }
    }

    /// Registers the engine component `C` under its persisted name and version, with its JSON
    /// Schema from `schemars` (whose description is the type's doc comment) and, for components
    /// scripts can name, its typed fields.
    pub fn register<C: Component + Persisted + JsonSchema>(
        world: &mut World,
        script: Option<ComponentSchema>,
    ) -> Result<ComponentId, Problem> {
        if !valid_component_name(C::NAME) {
            return Err(invalid_name("component", C::NAME, "^[A-Z][A-Za-z0-9]*$"));
        }
        let schema = schemars::schema_for!(C).to_value();
        let doc = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let id = world.register_component::<C>();
        Self::add(
            world,
            ComponentEntry {
                name: C::NAME.into(),
                origin: ComponentOrigin::Engine,
                version: C::VERSION,
                doc: doc.into(),
                id,
                type_id: Some(TypeId::of::<C>()),
                json_schema: Arc::new(schema),
                script: script.map(Arc::new),
            },
        )?;
        Ok(id)
    }

    /// Registers a project component the script host has registered with the world as a dynamic
    /// component of layout [`ProjectValues`] under `id`.
    pub fn register_project(
        world: &mut World,
        schema: ComponentSchema,
        id: ComponentId,
    ) -> Result<(), Problem> {
        let json = schema.json_schema();
        Self::add(
            world,
            ComponentEntry {
                name: schema.name.clone(),
                origin: ComponentOrigin::Project,
                version: schema.version,
                doc: schema.doc.clone(),
                id,
                type_id: None,
                json_schema: Arc::new(json),
                script: Some(Arc::new(schema)),
            },
        )
    }

    /// The entry named `name`.
    pub fn get(&self, name: &str) -> Option<&ComponentEntry> {
        self.entries
            .binary_search_by(|e| (*e.name).cmp(name))
            .ok()
            .map(|i| &self.entries[i])
    }

    /// The entry of a world component id.
    pub fn by_id(&self, id: ComponentId) -> Option<&ComponentEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Every entry, ascending by name.
    pub fn entries(&self) -> &[ComponentEntry] {
        &self.entries
    }

    /// "Did you mean" suggestions for an unknown component name.
    pub fn suggest(&self, unknown: &str) -> Vec<String> {
        suggest_names(unknown, self.entries.iter().map(|e| &*e.name))
    }

    /// Names, versions, docs and JSON Schemas, for presenters and tools.
    pub fn info(&self) -> Vec<ComponentInfo> {
        self.entries
            .iter()
            .map(|e| ComponentInfo {
                name: e.name.to_string(),
                origin: e.origin,
                version: e.version,
                doc: e.doc.to_string(),
                schema: (*e.json_schema).clone(),
            })
            .collect()
    }
}

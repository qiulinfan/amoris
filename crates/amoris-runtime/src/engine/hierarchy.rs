//! `Parent` for scripts (script-host.md 5.4 and 7.2): one entity field, the parent. An entity
//! without a parent has no `Parent`, so the slot's 0, which stands for no entity in every other
//! entity field, is refused at the script's calling line by the slot rule below
//! (`script.write_not_held`): a script makes an entity a root by removing the component. Whether
//! the parent is alive is the entity field's own check, and whether it closes a loop is the script
//! host's, which knows the hierarchy the call has made so far (amoris-script `natives/world.rs`).

use amoris_contract::Problem;
use amoris_script::{EngineFns, SlotRefusal, register_engine, register_engine_rule};
use amoris_sim::registry::{ComponentSchema, FieldType, FieldValue, ProjectValues};
use amoris_sim::{EntityId, Parent, Persisted};
use bevy_ecs::prelude::World;

use super::{nums, schema};

pub(super) fn parent_schema() -> Result<ComponentSchema, Problem> {
    schema(
        "Parent",
        Parent::VERSION,
        "Places an entity under another in the hierarchy; its Transform stays in world space. \
         Despawning the parent despawns the entity. Remove the component to make the entity a \
         root.",
        vec![(
            "parent",
            FieldType::Entity,
            FieldValue::Entity(None),
            "The parent entity.",
        )],
    )
}

fn parent_fns() -> EngineFns<Parent> {
    EngineFns {
        read: |p| nums(&[p.parent.to_f64()]),
        write: |p, v| {
            if let Ok(id) = EntityId::from_f64(v.nums[0]) {
                p.parent = id;
            }
        },
        make: |v| Parent {
            // The slot rule refuses a slot of 0 before anything is staged, and an entity field's
            // own check refuses every other number that is no id, so the fallback is never taken.
            parent: EntityId::from_f64(v.nums[0]).unwrap_or(EntityId::FIRST),
        },
    }
}

/// `Parent`'s slot rule: the parent is never none.
fn parent_rule(
    _before: Option<&ProjectValues>,
    after: &mut ProjectValues,
) -> Result<(), SlotRefusal> {
    if after.nums[0] == 0.0 {
        return Err(SlotRefusal {
            field: "parent",
            message: "Parent.parent is none: a Parent names an entity; remove the component to \
                      make the entity a root (ctx.world.remove(e, \"Parent\"))."
                .to_owned(),
        });
    }
    Ok(())
}

/// Registers the slot functions and the slot rule; the schema is in the registry already.
pub(super) fn register(world: &mut World) -> Result<(), Problem> {
    register_engine::<Parent>(world, parent_fns())?;
    register_engine_rule::<Parent>(world, parent_rule)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId::new(n).expect("an id")
    }

    #[test]
    fn the_slots_round_trip() {
        let fns = parent_fns();
        let schema = parent_schema().unwrap();
        let p = Parent { parent: id(41) };
        let v = (fns.read)(&p);
        assert_eq!(v.nums.len(), usize::from(schema.slots()));
        assert_eq!(*v.nums, [41.0]);
        assert_eq!((fns.make)(&v), p);
        let mut out = Parent { parent: id(1) };
        (fns.write)(&mut out, &v);
        assert_eq!(out, p);
        // A slot that is no id leaves the component as it was.
        (fns.write)(&mut out, &nums(&[0.0]));
        assert_eq!(out, p);
        assert_eq!(schema.fields[0].ty, FieldType::Entity);
    }

    #[test]
    fn the_rule_refuses_no_parent() {
        let schema = parent_schema().unwrap();
        // An insert or a spawn that gives no parent: the schema's default.
        let mut none = amoris_script::access::defaults(&schema);
        let r = parent_rule(None, &mut none).unwrap_err();
        assert_eq!(r.field, "parent");
        assert!(
            r.message.starts_with("Parent.parent is none"),
            "{}",
            r.message
        );
        // A set to null on an entity that has one.
        let before = nums(&[3.0]);
        assert_eq!(
            parent_rule(Some(&before), &mut nums(&[0.0]))
                .unwrap_err()
                .field,
            "parent"
        );
        // Any entity is held as given, and a write that changes nothing is accepted.
        let mut after = nums(&[7.0]);
        parent_rule(Some(&before), &mut after).unwrap();
        assert_eq!(*after.nums, [7.0]);
        parent_rule(Some(&before), &mut before.clone()).unwrap();
    }
}

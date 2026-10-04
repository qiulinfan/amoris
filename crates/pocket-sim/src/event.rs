//! Events (docs/spec/simulation.md 5): records that systems and boundary writes emit, numbered by
//! a world-wide sequence and delivered one tick later to every reader. During tick n systems read
//! the inbox (tick n-1's events and the boundary writes' since) and append to the outbox;
//! `sim.finish` makes the outbox the new inbox. The inbox and the counter are persisted and hashed
//! with the world, so a divergence in events shows in the hash of the tick that emitted them.

use bevy_ecs::prelude::{Res, ResMut, Resource, World};
use bevy_ecs::system::SystemParam;
use pocket_contract::{Problem, detail};
use schemars::JsonSchema;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::data::PlainData;
use crate::entity::EntityId;
use crate::sim::TickState;
use crate::time::{SimClock, Tick};

/// A world-wide sequence number: allocated when an event is appended, never reused, at most
/// 2^53 - 1.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize, JsonSchema,
)]
pub struct EventSeq(pub u64);

/// An event's type: dotted snake_case with at least one dot, `contact.begin`, `crate.taken`
/// (`^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$`, script-host.md 5.5). Decoding checks it too.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, JsonSchema)]
pub struct EventKind(String);

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EventKind, D::Error> {
        /// The encoded shape, `EventKind`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "EventKind")]
        struct Repr(String);
        let Repr(kind) = Repr::deserialize(d)?;
        EventKind::new(kind).map_err(de::Error::custom)
    }
}

impl EventKind {
    /// A kind, or `sim.event_kind_invalid`.
    pub fn new(kind: impl Into<String>) -> Result<EventKind, Problem> {
        let kind = kind.into();
        let word = |w: &str| {
            let mut b = w.bytes();
            matches!(b.next(), Some(c) if c.is_ascii_lowercase())
                && b.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        };
        if kind.contains('.') && kind.split('.').all(word) {
            Ok(EventKind(kind))
        } else {
            Err(Problem::new(
                "sim.event_kind_invalid",
                format!(
                    "'{kind}' is not an event kind: use dotted snake_case words such as \
                     'crate.taken'."
                ),
                detail([("kind", json!(kind))]),
            ))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub seq: EventSeq,
    /// The clock when it was appended: n during tick n, n-1 at the boundary before tick n.
    pub tick: Tick,
    pub kind: EventKind,
    pub subject: Option<EntityId>,
    /// The event that led to this one, when the emitter names it.
    pub cause: Option<EventSeq>,
    pub data: PlainData,
}

/// The sequence counter, a persisted resource: the number the next event receives, 1 in a new
/// world. Decoding refuses a `next` outside 1 to 2^53.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Serialize, JsonSchema)]
pub struct EventCounter {
    next: u64,
}

impl<'de> Deserialize<'de> for EventCounter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EventCounter, D::Error> {
        /// The encoded shape, `EventCounter`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "EventCounter")]
        struct Repr {
            next: u64,
        }
        let Repr { next } = Repr::deserialize(d)?;
        if !(1..=crate::MAX_ENTITY_ID + 1).contains(&next) {
            return Err(de::Error::custom(format!(
                "the event counter's next number {next} is outside 1 to 2^53"
            )));
        }
        Ok(EventCounter { next })
    }
}

impl Default for EventCounter {
    fn default() -> Self {
        EventCounter { next: 1 }
    }
}

impl EventCounter {
    /// The number the next event receives.
    pub fn next(&self) -> u64 {
        self.next
    }

    fn take(&mut self) -> EventSeq {
        let s = EventSeq(self.next);
        self.next += 1;
        s
    }

    pub(crate) fn reset_to(&mut self, next: u64) {
        self.next = next;
    }
}

/// The events the next tick reads, a persisted resource: after tick n, the events tick n emitted
/// followed by those the boundary writes since appended (from `boundary_from` on), ascending by
/// sequence. Decoding refuses a `boundary_from` past the events, sequence numbers that do not
/// ascend, and event data not in its canonical form.
#[derive(Resource, Clone, Debug, Default, PartialEq, Serialize, JsonSchema)]
pub struct EventInbox {
    events: Vec<Event>,
    boundary_from: u32,
}

impl<'de> Deserialize<'de> for EventInbox {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EventInbox, D::Error> {
        /// The encoded shape, `EventInbox`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "EventInbox")]
        struct Repr {
            events: Vec<Event>,
            boundary_from: u32,
        }
        let Repr {
            events,
            boundary_from,
        } = Repr::deserialize(d)?;
        if !usize::try_from(boundary_from).is_ok_and(|b| b <= events.len()) {
            return Err(de::Error::custom(format!(
                "the inbox's boundary mark {boundary_from} is past its {} events",
                events.len()
            )));
        }
        if let Some(w) = events.windows(2).find(|w| w[0].seq >= w[1].seq) {
            return Err(de::Error::custom(format!(
                "the inbox's event {} follows event {}: sequence numbers must ascend",
                w[1].seq.0, w[0].seq.0
            )));
        }
        for e in &events {
            e.data.validate().map_err(de::Error::custom)?;
        }
        Ok(EventInbox {
            events,
            boundary_from,
        })
    }
}

impl EventInbox {
    /// Every event in the inbox, in sequence order.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Where the boundary writes' events start.
    pub fn boundary_from(&self) -> usize {
        self.boundary_from as usize
    }

    /// The events the last tick emitted (the tick's event record, simulation.md 5.3).
    pub fn tick_events(&self) -> &[Event] {
        &self.events[..self.boundary_from()]
    }

    /// The events boundary writes appended since the last tick.
    pub fn boundary_events(&self) -> &[Event] {
        &self.events[self.boundary_from()..]
    }

    /// The events of one kind, in sequence order.
    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Event> + 'a {
        self.events.iter().filter(move |e| e.kind.as_str() == kind)
    }

    pub(crate) fn replace_with_tick(&mut self, events: Vec<Event>) {
        self.boundary_from = u32::try_from(events.len()).unwrap_or(u32::MAX);
        self.events = events;
    }

    pub(crate) fn push_boundary(&mut self, e: Event) {
        self.events.push(e);
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.events.truncate(len);
    }
}

/// The events appended during the running tick. Derived: empty at every boundary, never persisted.
#[derive(Resource, Debug, Default)]
pub struct EventOutbox {
    events: Vec<Event>,
}

impl EventOutbox {
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub(crate) fn take(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.events.truncate(len);
    }

    pub(crate) fn push(&mut self, e: Event) {
        self.events.push(e);
    }
}

/// What an emitter gives: everything but the sequence number and the tick.
#[derive(Clone, Debug, PartialEq)]
pub struct NewEvent {
    pub kind: EventKind,
    pub subject: Option<EntityId>,
    pub cause: Option<EventSeq>,
    pub data: PlainData,
}

impl NewEvent {
    /// An event of `kind` with no subject, cause or data.
    pub fn new(kind: EventKind) -> NewEvent {
        NewEvent {
            kind,
            subject: None,
            cause: None,
            data: PlainData::Null,
        }
    }

    pub fn subject(mut self, subject: EntityId) -> NewEvent {
        self.subject = Some(subject);
        self
    }

    pub fn cause(mut self, cause: EventSeq) -> NewEvent {
        self.cause = Some(cause);
        self
    }

    pub fn data(mut self, data: PlainData) -> NewEvent {
        self.data = data;
        self
    }
}

/// Appends an event: to the outbox during a tick (read by the next tick), to the inbox at a
/// boundary (read by the next tick too). Its sequence number is allocated now.
pub fn emit(world: &mut World, event: NewEvent) -> EventSeq {
    let tick = world.resource::<SimClock>().tick;
    let seq = world.resource_mut::<EventCounter>().take();
    let e = Event {
        seq,
        tick,
        kind: event.kind,
        subject: event.subject,
        cause: event.cause,
        data: event.data,
    };
    if world.resource::<TickState>().running().is_some() {
        world.resource_mut::<EventOutbox>().push(e);
    } else {
        world.resource_mut::<EventInbox>().push_boundary(e);
    }
    seq
}

/// Emitting from an engine system: `fn contacts(mut emit: Emit, ...)`.
#[derive(SystemParam)]
pub struct Emit<'w> {
    counter: ResMut<'w, EventCounter>,
    outbox: ResMut<'w, EventOutbox>,
    clock: Res<'w, SimClock>,
}

impl Emit<'_> {
    /// Appends an event to the outbox; the next tick reads it.
    pub fn emit(&mut self, event: NewEvent) -> EventSeq {
        let seq = self.counter.take();
        self.outbox.push(Event {
            seq,
            tick: self.clock.tick,
            kind: event.kind,
            subject: event.subject,
            cause: event.cause,
            data: event.data,
        });
        seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert!(EventKind::new("contact.begin").is_ok());
        assert!(EventKind::new("crate.taken_2.x").is_ok());
        for bad in [
            "contact",
            "Contact.begin",
            "contact.",
            ".begin",
            "a..b",
            "a.2b",
            "",
        ] {
            assert_eq!(
                EventKind::new(bad).unwrap_err().code,
                "sim.event_kind_invalid",
                "{bad}"
            );
        }
    }
}

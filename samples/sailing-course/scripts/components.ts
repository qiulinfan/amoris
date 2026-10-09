// The sailing course's own components (script-host.md 7.3): the crew, the tally and the cargo of
// the sailing sample, and the course with its marks. Every bit of game state is a component, so a
// reload, a fork or a replay carries it (charter 3.2). The perception declarations
// (perception.json) read Mark.color, Mark.round_to, Mark.order, Course.next and Tally.taken.
import { component, field } from "pocket";

export const Crew = component("Crew", {
    version: 1, doc: "The boat's crew and what they are told to do.",
    fields: {
        take: field.entity("A crate to take aboard at the next tick (the interact pulse); cleared once the crew tried."),
    },
});

export const Tally = component("Tally", {
    version: 1, doc: "Crates aboard and crates there were.",
    fields: {
        taken: field.u32(0, "Crates taken aboard."),
        worth: field.u32(0, "The value of the crates aboard."),
        total: field.u32(0, "Crates adrift when the game began."),
    },
});

export const Cargo = component("Cargo", {
    version: 1, doc: "A crate adrift that a crew can take aboard.",
    fields: {
        value: field.u32(1, "What it is worth."),
    },
});

export const Mark = component("Mark", {
    version: 1, doc: "A course mark: rounded in order.",
    fields: {
        order: field.u32(1, "Its place in the course, from 1."),
        label: field.str("", "Its name on the chart, as mark.rounded reports it."),
        color: field.str("yellow", "Its colour."),
        round_to: field.enum(["port", "starboard"], "port", "The side to leave it on."),
    },
});

export const Course = component("Course", {
    version: 1, doc: "The course: the next mark to round and how many there are.",
    fields: {
        next: field.u32(1, "The order of the mark to round next; past the last when finished."),
        marks: field.u32(0, "Marks in the course."),
        finished: field.bool(false, "Whether the course is finished."),
        finished_tick: field.tick(0, "The tick the course was finished in."),
    },
});

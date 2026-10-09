// The sailing course's rules: stateless systems over the world (charter 3.2). The boat is the
// engine's Boat, which the player's controls and intents drive (pocket-interface); these rules
// count the crates and the marks, take a crate aboard when the crew is told to (the interact
// pulse writes Crew.take), and round the marks in order.
import { system } from "pocket";

/** How near a crate must float, across the water, to be taken aboard (master's REACH). */
const REACH = 3;
/** How far above or below. */
const REACH_UP = 3;
/** A mark is rounded when the boat comes within this many metres of it (its side is not judged). */
const ROUND_M = 15;

export const muster = system({
    name: "muster", phase: "update", doc: "Counts the crates adrift into every boat's tally and the marks into the course.",
    when: "start",
    queries: { boats: { with: ["Tally"] }, crates: { with: ["Cargo"] }, courses: { with: ["Course"] }, marks: { with: ["Mark"] } },
    run(ctx, { boats, crates, courses, marks }) {
        boats.each((_r, e) => ctx.world.set(e, "Tally", { total: crates.len }));
        courses.each((_r, e) => ctx.world.set(e, "Course", { marks: marks.len }));
    },
});

export const takeAboard = system({
    name: "take_aboard", phase: "update",
    doc: "A crew told to take a crate takes it aboard when it floats within reach, and says why not otherwise.",
    queries: { boats: { with: ["Crew", "Tally", "Transform"], fields: ["Crew.take", "Transform.position", "Tally.taken", "Tally.worth", "Tally.total"] } },
    run(ctx, { boats }) {
        const crew = boats.cols.Crew;
        const at = boats.cols.Transform.position;
        const tally = boats.cols.Tally;
        boats.each((r, boat) => {
            const target = crew.take[r];
            if (target === 0) return;
            crew.take[r] = 0;
            const crate = target;
            const cargo = ctx.world.get(crate, "Cargo");
            const place = ctx.world.get(crate, "Transform");
            if (!cargo || !place) {
                ctx.emit("interact.ignored", { code: "sail.crate_gone", message: "The crate was no longer there to take aboard." }, { subject: boat });
                return;
            }
            const p = place.position;
            const across = Math.hypot(p.x - at.x[r], p.z - at.z[r]);
            if (across > REACH || Math.abs(p.y - at.y[r]) > REACH_UP) {
                ctx.emit("interact.ignored", { code: "sail.out_of_reach", message: "The crate was out of reach when the crew tried." }, { subject: boat });
                return;
            }
            ctx.world.despawn(crate);
            tally.taken[r] = tally.taken[r] + 1;
            tally.worth[r] = tally.worth[r] + cargo.value;
            const left = tally.total[r] - tally.taken[r];
            ctx.emit("crate.taken", { crate, taken: tally.taken[r], left }, { subject: boat });
        });
    },
});

export const rounding = system({
    name: "rounding", phase: "update",
    doc: "A boat that comes within 15 m of the course's next mark rounds it; the last one rounded finishes the course.",
    queries: {
        boats: { with: ["Tally", "Transform"], fields: ["Transform.position"] },
        courses: { with: ["Course"], fields: ["Course.next", "Course.marks", "Course.finished", "Course.finished_tick"] },
        marks: { with: ["Mark", "Transform"], fields: ["Mark.order", "Transform.position"] },
    },
    run(ctx, { boats, courses, marks }) {
        const c = courses.cols.Course;
        const order = marks.cols.Mark.order;
        const where = marks.cols.Transform.position;
        const at = boats.cols.Transform.position;
        courses.each((cr) => {
            if (c.finished[cr] === 1) return;
            let m = -1;
            marks.each((mr) => {
                if (m < 0 && order[mr] === c.next[cr]) m = mr;
            });
            if (m < 0) return;
            boats.each((br, boat) => {
                if (c.finished[cr] === 1 || order[m] !== c.next[cr]) return;
                const d = Math.hypot(where.x[m] - at.x[br], where.z[m] - at.z[br]);
                if (d > ROUND_M) return;
                const mark = ctx.world.get(marks.id(m), "Mark");
                ctx.emit("mark.rounded", { mark: mark ? mark.label : "", order: order[m] }, { subject: boat });
                c.next[cr] = c.next[cr] + 1;
                if (c.next[cr] > c.marks[cr]) {
                    c.finished[cr] = 1;
                    c.finished_tick[cr] = ctx.tick;
                    ctx.emit("course.finished", { marks_rounded: c.marks[cr] }, { subject: boat });
                }
            });
        });
    },
});

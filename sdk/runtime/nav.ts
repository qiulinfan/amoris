// Navigation (docs/design/navigation.md): a walkability grid baked from the static colliders or a
// tile map, and A* paths over it, for scripts that steer things and agents that ask "can it get
// there" and "which way". Every call is a runtime command.
import { repro } from "./repro";
import { command, world, type EntityRef, type Vec3 } from "./world";

export interface NavPath {
    /** From the start cell's center to the goal's; y is the ground height (or the map's plane). */
    points: Vec3[];
    length: number;
    /** The goal could not be reached: the path ends at the closest cell. */
    partial: boolean;
    /** The start or the goal was off walkable ground and moved to the nearest cell. */
    snapped: boolean;
    /** Nodes A* expanded: cells, or polygons on a navmesh path. */
    expanded: number;
    /** Cells along the path before smoothing (0 on a navmesh path). */
    cells: number;
    /** Found over the navmesh (else over the cells). */
    mesh: boolean;
    /** Polygons crossed on a navmesh path. */
    polys: number;
}

export interface NavInfo {
    baked: boolean;
    plane?: "xz" | "xy";
    width?: number;
    height?: number;
    cell?: number;
    origin?: Vec3;
    cells?: number;
    walkable?: number;
    links?: number;
    diagonal?: boolean;
    max_step?: number;
    /** Floors in the busiest column (1 on tile grids and single-surface levels). */
    layers?: number;
    source?: string;
    baked_tick?: number;
    /** The navmesh over the grid: rectangles of cells and the edges they share (none on platformer grids). */
    mesh?: { polygons: number; portals: number };
    /** The radius the grid was baked for; obstacles grow by it. */
    agent_radius?: number;
    /** NavObstacle entities applied this tick and the cells under them. */
    obstacles: number;
    /** Obstacles with a velocity this tick (a Velocity component, or a position that changed since the last tick). */
    moving_obstacles: number;
    blocked: number;
    /** NavAgent entities moved this tick, by state, and how many replanned or deviated to avoid something. */
    agents: number;
    moving: number;
    arrived: number;
    stuck: number;
    replans: number;
    avoiding: number;
    /** Agents that slowed down behind another this tick (their `queue` preference). */
    queuing: number;
    /** Followers walking a path to a slot out of sight (behind a wall) this tick. */
    detours: number;
}

export interface NavAgentInfo {
    id: number;
    path: string;
    mode: number;
    /** 0 idle, 1 moving, 2 arrived, 3 stuck. */
    state: number;
    speed: number;
    radius: number;
    velocity: Vec3;
    /** The point the agent heads for: the next corner of its path or the goal. */
    corner: Vec3;
    /** Length of the remaining path. */
    distance: number;
    neighbours: number;
    /** Mode 3: the slot relative to the leader (x along its heading, z or y to its right). */
    offset: Vec3;
    queue: number;
    priority: number;
    /** Whether the agent slowed down behind another this tick. */
    queued: boolean;
    /** Corners left on the planned path (absent when the agent has no plan). */
    corners?: number;
    partial?: boolean;
    planned_tick?: number;
}

export type NavPoint = Vec3 | [number, number, number] | EntityRef;

/** The NavAgent fields a walk, follow or slot may set alongside its mode. */
export interface AgentOptions {
    speed?: number;
    radius?: number;
    arrive?: number;
    replan?: number;
    avoidance?: number;
    queue?: number;
    priority?: number;
}

function entityId(ref: EntityRef): number {
    return typeof ref === "number" ? ref : (world.find(ref) ?? 0);
}

export const nav = {
    /**
     * Bake a grid over the XZ rectangle from the static colliders: a cell is walkable where a ray
     * down finds static ground flatter than max_slope with room for the agent. Every floor of a
     * column counts, up to `layers` (4; 1 for the top surface only): a bridge and the road under it.
     */
    bake(options: { min: Vec3; max: Vec3; cell?: number; agent_radius?: number; agent_height?: number; max_step?: number; max_slope?: number; diagonal?: boolean; layers?: number }): NavInfo {
        return command("nav.bake", options);
    },
    /** Bake from a TileMap entity: "topdown" walks every empty cell, "platformer" links standing cells by jumps and drops. */
    bakeTilemap(entity: EntityRef, options: { mode?: "topdown" | "platformer"; agent_height?: number; jump_height?: number; jump_range?: number; max_drop?: number; diagonal?: boolean } = {}): NavInfo {
        return command("nav.bake", { entity, ...options });
    },
    /** A* between two points or entities, string-pulled unless smooth is false. Throws when nothing is baked or a point is outside. */
    path(from: NavPoint, to: NavPoint, options: { smooth?: boolean; mesh?: boolean } = {}): NavPath {
        return command("nav.path", { from, to, ...options });
    },
    /** The navmesh over the grid: its rectangles in world space with their neighbours (docs/design/navigation.md, Navmesh). */
    mesh(): { count: number; portals: number; polygons: Array<{ id: number; cells: number; min: Vec3; max: Vec3; neighbours: number[] }> } {
        return command("nav.mesh");
    },
    /** Both on walkable ground with a complete path between them. */
    reachable(from: NavPoint, to: NavPoint): boolean {
        return (command("nav.reachable", { from, to }) as { reachable: boolean }).reachable;
    },
    /** The nearest walkable cell center within radius, or undefined. */
    nearest(point: NavPoint, radius = 2): Vec3 | undefined {
        const r = command<Vec3 | null>("nav.nearest", { point, radius });
        return r === null ? undefined : r;
    },
    info(): NavInfo {
        return command("nav.info");
    },
    /** Every NavAgent with its state and plan, ordered by entity id (docs/design/navigation.md, Agents). */
    /** Send an entity walking to a point (it gets a NavAgent in mode 1; the options are the agent's fields). */
    walk(entity: EntityRef, goal: Vec3, options: AgentOptions = {}): void {
        world.set(entity, "NavAgent", { ...options, mode: 1, goal });
    },
    /** Make an entity follow another (mode 2). */
    follow(entity: EntityRef, target: EntityRef, options: AgentOptions = {}): void {
        world.set(entity, "NavAgent", { ...options, mode: 2, target: entityId(target) });
    },
    /**
     * Keep a slot beside a leader (mode 3): `forward` along the leader's heading (negative is
     * behind), `side` to its right; the follower matches the leader's speed and steers to the slot.
     */
    slot(entity: EntityRef, leader: EntityRef, offset: { forward: number; side: number }, options: AgentOptions = {}): void {
        const xy = nav.info().plane === "xy";
        world.set(entity, "NavAgent", { ...options, mode: 3, target: entityId(leader), offset: { x: offset.forward, y: xy ? offset.side : 0, z: xy ? 0 : offset.side } });
    },
    /**
     * Put a group in formation behind or around a leader: a column (one behind the other), a line
     * (beside the leader, right then left), a wedge (a V behind it) or a circle around it, `spacing`
     * apart (default 1). Returns each member's slot.
     */
    formation(leader: EntityRef, members: EntityRef[], options: AgentOptions & { shape?: "column" | "line" | "wedge" | "circle"; spacing?: number } = {}): Array<{ entity: EntityRef; forward: number; side: number }> {
        const { shape = "column", spacing = 1, ...agent } = options;
        const n = members.length;
        const slots = members.map((entity, i) => {
            const k = Math.ceil((i + 1) / 2), sign = i % 2 === 0 ? 1 : -1;
            if (shape === "line") return { entity, forward: 0, side: sign * k * spacing };
            if (shape === "wedge") return { entity, forward: -k * spacing, side: sign * k * spacing };
            if (shape === "circle") { const a = (2 * Math.PI * (i + 1)) / (n + 1); return { entity, forward: repro.cos(a) * spacing, side: repro.sin(a) * spacing }; }
            return { entity, forward: -(i + 1) * spacing, side: 0 };
        });
        for (const s of slots) nav.slot(s.entity, leader, { forward: s.forward, side: s.side }, agent);
        return slots;
    },
    /** Leave the entity where it is (mode 0). */
    stop(entity: EntityRef): void {
        world.set(entity, "NavAgent", { mode: 0 });
    },
    agents(): NavAgentInfo[] {
        return command("nav.agents");
    },
    clear(): void {
        command("nav.clear");
    },
};

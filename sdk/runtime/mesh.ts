// Meshes made by code (docs/design/assets.md, Meshes made by code): vertices and triangles as
// numbers, drawn by a MeshRenderer and collided with by a mesh Collider as "mesh:<name>".
import { command } from "./world";

type Vec3Like = number[] | { x: number; y: number; z: number };

export interface MeshData {
    /** Three numbers a vertex: flat, [x, y, z] or {x, y, z}. */
    positions: number[] | Vec3Like[];
    /** Three a triangle, counter-clockwise seen from its front; without them every three positions make one. */
    indices?: number[] | number[][];
    /** Made from the triangles when left out: vertices the triangles share come out smooth, separate ones flat. */
    normals?: number[] | Vec3Like[];
    /** Two numbers a vertex. */
    uvs?: number[] | number[][];
    /** sRGB, three or four numbers a vertex, multiplying the MeshRenderer's color. */
    colors?: number[] | number[][];
    double_sided?: boolean;
}

export interface MadeMesh {
    /** What MeshRenderer.mesh and Collider.mesh name: "mesh:<name>". */
    mesh: string;
    vertices: number;
    triangles: number;
    bounds: { min: [number, number, number]; max: [number, number, number] };
}

export const meshes = {
    /** Make (or replace) a mesh from numbers; draw it with MeshRenderer { mesh: "mesh:<name>" }. */
    create(name: string, data: MeshData): MadeMesh {
        return command("mesh.create", { name, ...data }) as MadeMesh;
    },
    /** The meshes made so far. */
    list(): Array<{ mesh: string; entries: { positions: number; indices: number } }> {
        return (command("mesh.list", {}) as { meshes: Array<{ mesh: string; entries: { positions: number; indices: number } }> }).meshes;
    },
    /** Forget a made mesh. */
    remove(name: string): void {
        command("mesh.remove", { name });
    },
};

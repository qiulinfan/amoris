// Icons by meaning: an entity's icon from its components (as Unity and Godot show node types), a
// component's icon in the inspector, and an asset's icon by kind.

import {
  Anchor,
  Box,
  BrainCircuit,
  Camera,
  CircleDot,
  Component,
  Cuboid,
  File,
  FileCode,
  Flag,
  Globe,
  Image,
  Lightbulb,
  Link,
  Move3d,
  Music,
  Package,
  Sailboat,
  Shapes,
  Sparkles,
  Sun,
  Users,
  Gauge,
  NotebookPen,
  WavesHorizontal,
  Wind,
  Weight,
  type LucideIcon,
} from "lucide-react";

const COMPONENT_ICONS: Record<string, LucideIcon> = {
  Transform: Move3d,
  Velocity: Gauge,
  RigidBody: Weight,
  Collider: Shapes,
  Boat: Sailboat,
  Sail: Sailboat,
  Sea: WavesHorizontal,
  Wind: Wind,
  Model: Box,
  Light: Lightbulb,
  Camera: Camera,
  Parent: Link,
  Cargo: Package,
  Crew: Users,
  Tally: Flag,
  Log: NotebookPen,
  Floater: Anchor,
  Splat: Sparkles,
};

export function componentIcon(name: string): LucideIcon {
  return COMPONENT_ICONS[name] ?? Component;
}

/** The component that best says what an entity is, in priority order. */
const PRIORITY = ["Camera", "Light", "Boat", "Sea", "Wind", "Splat", "Cargo", "Model", "Collider", "RigidBody"];

export function entityKind(components: string[], lightKind?: string): { icon: LucideIcon; tone: string } {
  const c = PRIORITY.find((p) => components.includes(p));
  switch (c) {
    case "Camera":
      return { icon: Camera, tone: "tone-camera" };
    case "Light":
      return { icon: lightKind === "Directional" ? Sun : Lightbulb, tone: "tone-light" };
    case "Boat":
      return { icon: Sailboat, tone: "tone-actor" };
    case "Sea":
      return { icon: WavesHorizontal, tone: "tone-env" };
    case "Wind":
      return { icon: Wind, tone: "tone-env" };
    case "Splat":
      return { icon: Sparkles, tone: "tone-mesh" };
    case "Cargo":
      return { icon: Package, tone: "tone-mesh" };
    case "Model":
      return { icon: Cuboid, tone: "tone-mesh" };
    case "Collider":
    case "RigidBody":
      return { icon: Shapes, tone: "tone-mesh" };
    default:
      return { icon: components.length <= 1 ? CircleDot : Globe, tone: "tone-empty" };
  }
}

const ASSET_ICONS: Record<string, LucideIcon> = {
  model: Box,
  texture: Image,
  splat: Sparkles,
  neural: BrainCircuit,
  audio: Music,
  script: FileCode,
  scene: Globe,
};

export function assetIcon(kind: string): LucideIcon {
  return ASSET_ICONS[kind] ?? File;
}

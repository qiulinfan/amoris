// Sound for gameplay code. Everything here is a command, so agents can play and inspect sound too.
declare const __pocket: { command(name: string, params?: unknown): unknown };

export interface PlayOptions {
    volume?: number;
    pitch?: number;
    loop?: boolean;
    pan?: number;
    /** How much of the high end is kept, 0..1: 1 is the clip as it is, small values muffle it (underwater, behind a door). */
    lowpass?: number;
    /** How much of the voice goes to the room's reverb (`audio.reverb`), 0..1; 0 keeps interface clicks and music dry. */
    reverb?: number;
    entity?: number;
    tag?: string;
    /** The bus it plays on (`main` by default): music, effects, dialogue, any name; see `audio.bus`. */
    bus?: string;
    /** Heard from where `entity` is: full within `near` of the camera, silent at `range`, panned to its side (docs/design/audio.md). */
    spatial?: boolean;
    near?: number;
    range?: number;
    /** How much a wall between the listener and `entity` takes, 0..1: a collider across the line scales the volume by 1 - occlusion and muffles the voice by the same (docs/design/audio.md, Where a sound is). */
    occlusion?: number;
    /** How strongly the pitch follows `entity`'s motion toward or away from the listener, and the listener's (the Doppler effect): 1 as in air (the default), 0 keeps the pitch. */
    doppler?: number;
}

export interface Voice {
    id: number;
    clip: string;
    position: number;
    duration: number;
    volume: number;
    pitch: number;
    pan: number;
    loop: boolean;
    entity: number;
    tag: string;
    bus: string;
    loops_done: number;
}

export interface BusSettings {
    /** 0..2. */
    volume?: number;
    muted?: boolean;
    /** Over the bus's whole mix, 0..1: 1 as it is, small values muffle it (a pause menu). */
    lowpass?: number;
    /** Another bus: while a voice plays on it, this one falls to `duck_amount` (music under dialogue); "" for none. */
    duck_by?: string;
    duck_amount?: number;
    /** How long the fall and the recovery take. */
    duck_seconds?: number;
}

export interface Bus extends Required<BusSettings> {
    name: string;
    /** The ducking gain right now, 1 when not ducked. */
    duck: number;
    ducked: boolean;
    voices: number;
}

function cmd<T>(name: string, params?: unknown): T {
    return __pocket.command(name, params) as T;
}

/** The AudioListener component (docs/design/audio.md, Where a sound is): spatial sounds are heard from its entity instead of the camera while it is enabled. */
export const audio = {
    /** Start a clip (project-relative WAV or Ogg Vorbis path); returns the voice id. */
    play(clip: string, options: PlayOptions = {}): number {
        return cmd<{ voice: number }>("audio.play", { clip, ...options }).voice;
    },
    /** Stop by voice id, by clip path, by tag, by bus, or everything. Returns how many voices stopped. */
    stop(target?: number | { clip?: string; tag?: string; bus?: string }): number {
        const params = target === undefined ? { all: true } : typeof target === "number" ? { voice: target } : target;
        return cmd<{ stopped: number }>("audio.stop", params).stopped;
    },
    set(voice: number, params: { volume?: number; pitch?: number; pan?: number; loop?: boolean; lowpass?: number; reverb?: number; bus?: string }): void {
        cmd("audio.set", { voice, ...params });
    },
    /** A bus's settings, changed by the fields given (made on first use); project.toml [audio.buses] sets the defaults. */
    bus(name: string, settings: BusSettings = {}): Bus {
        return cmd<Bus>("audio.bus", { name, ...settings });
    },
    buses(): Bus[] {
        return cmd<Bus[]>("audio.buses");
    },
    /**
     * The room every voice plays in: `room` is how long the tail rings (0 none, 0.5 a room, 0.9 a hall),
     * `damping` how fast its high end dies, `mix` its level against the dry voices. A voice's `reverb`
     * is its share. project.toml [audio.reverb] sets the default.
     */
    reverb(settings: { room?: number; damping?: number; mix?: number } = {}): { room: number; damping: number; mix: number } {
        return cmd("audio.reverb", settings);
    },
    voices(): Voice[] {
        return cmd<Voice[]>("audio.list");
    },
    clips(): Array<{ path: string; seconds: number }> {
        return cmd("audio.clips");
    },
    stats(): { device: string; sample_rate: number; voices: number; clips: number; plays: number; master_volume: number; muted: boolean } {
        return cmd("audio.stats");
    },
    setMasterVolume(volume: number): void {
        cmd("audio.master", { volume });
    },
    mute(on = true): void {
        cmd("audio.master", { muted: on });
    },
};

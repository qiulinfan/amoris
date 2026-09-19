// Sound for gameplay code. Everything here is a command, so agents can play and inspect sound too.
declare const __pocket: { command(name: string, params?: unknown): unknown };

export interface PlayOptions {
    volume?: number;
    pitch?: number;
    loop?: boolean;
    pan?: number;
    entity?: number;
    tag?: string;
    /** Heard from where `entity` is: full within `near` of the camera, silent at `range`, panned to its side (docs/design/audio.md). */
    spatial?: boolean;
    near?: number;
    range?: number;
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
    loops_done: number;
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
    /** Stop by voice id, by clip path, by tag, or everything. Returns how many voices stopped. */
    stop(target?: number | { clip?: string; tag?: string }): number {
        const params = target === undefined ? { all: true } : typeof target === "number" ? { voice: target } : target;
        return cmd<{ stopped: number }>("audio.stop", params).stopped;
    },
    set(voice: number, params: { volume?: number; pitch?: number; pan?: number; loop?: boolean }): void {
        cmd("audio.set", { voice, ...params });
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

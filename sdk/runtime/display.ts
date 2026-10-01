// The game's window (docs/design/input.md, The window): fullscreen, size and title, from a script
// as an agent asks with window.info and window.set.
import { command } from "./world";

export interface WindowInfo {
    width: number;
    height: number;
    pixel_width: number;
    pixel_height: number;
    pixel_density: number;
    fullscreen: boolean;
    title: string;
    headless: boolean;
}

export const display = {
    /** The window as it is: its size in points and pixels, fullscreen, title. */
    info(): WindowInfo {
        return command<WindowInfo>("window.info", {});
    },
    /** Fullscreen on or off (a browser grants it only while handling a click or a key press, so call it from an input handler there). */
    fullscreen(on: boolean): WindowInfo {
        return command<WindowInfo>("window.set", { fullscreen: on });
    },
    /** The window's size in points when not fullscreen. */
    resize(width: number, height: number): WindowInfo {
        return command<WindowInfo>("window.set", { width, height });
    },
    title(title: string): WindowInfo {
        return command<WindowInfo>("window.set", { title });
    },
};

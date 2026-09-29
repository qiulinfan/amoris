// Lockstep games (docs/design/networking.md): every peer runs the whole game and each tick waits
// for every player's input. Gameplay reads each player's actions with input.axis(name, player);
// net.info() says which player this peer is (for its camera and its interface, never for the
// game's own logic, which every peer must run the same).
declare const __pocket: { command(name: string, params?: unknown): unknown };

export interface NetInfo {
    /** "off" without a network game, "host" (player 0) or "player". */
    mode: "off" | "host" | "player";
    /** This peer's player number. */
    player: number;
    players: number;
    started: boolean;
    delay?: number;
    port?: number;
    connected?: number;
    desyncs?: number;
    waiting?: boolean;
    tick?: number;
}

export const net = {
    info(): NetInfo {
        return __pocket.command("net.info") as NetInfo;
    },
    /** This peer's player number (0 without a network game). */
    local(): number {
        return (__pocket.command("net.info") as NetInfo).player;
    },
};

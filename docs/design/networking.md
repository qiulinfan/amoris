# Networking

Pocket's multiplayer is lockstep. Every peer runs the whole game from the same scene and the same
seed, and a tick runs only once every player's input for that tick has arrived, so every peer
computes the same world without sending any of it. The engine was already deterministic per tick
(the same inputs make the same state hash, which is what journals, scenarios and replays rely on);
lockstep turns that into a network game, and the state hash turns into a check that it stays one
game.

## Playing

```bash
pocket run arena -- --net-host 7777                  # player 0, waits for one more
pocket run arena -- --net-join 192.168.1.20:7777     # player 1
```

`--net-host PORT` (0 for any free port) listens for `--net-players` players in all, itself included
(2 by default); `--net-join HOST:PORT` (or `ws://HOST:PORT/`, over a WebSocket) connects to a host,
takes the player number it is given and the host's seed (before anything is made from it, so the
scene, the scripts' `onStart` and every random number are the same on every peer) and the host's
input delay (`--net-delay`, 3 ticks). Until every player is in, the peers draw and wait: no tick
runs, and those frames do not count against `--frames`. Then all of them start at tick 0.

## A tick

A peer's own input (its keys, pad, mouse, and the synthetic input of `input.hold`, `input.press`,
`input.pad`) does not act at once: it waits until the peer is about to run a tick T and goes out for
tick T + delay, to everyone through the host. Tick T runs when every player's input for T is in; the
first `delay` ticks have none to wait for. Then each player's input goes through that player's own
input map (player 0's is the ordinary one), the scripts' `onInput` receives every player's events
tagged with `player`, and the tick runs. A peer ahead of the others waits, drawing, until their
input arrives; with the delay covering the round trip, nobody waits at all.

Gameplay reads each player's actions by number: `input.axis("move_x", p)`, `input.down("jump", p)`,
`input.pressed(...)`, `input.released(...)` (the tick's `players` array holds them all). Without a
network game only player 0 has input and every other player's actions are at rest, so a lockstep
game plays alone too. `net.info()` (the `net.info` command) says which player this peer is
(`player`), whether the game started, the delay, who is connected and who left, and whether the next
tick is waiting; use it for the camera and the interface only, since the game's own logic must run
the same on every peer.

## Staying one game

Every thirtieth tick each peer reports its world's hash (the world state and the particles) for the
tick just run; the host compares them and, when they differ, emits `net.desync` with the tick and
every peer's hash into the event log, sends it to everyone, and counts it in `net.info` (`desyncs`,
`first_desync`). A desync means something outside the lockstep changed a world: a command that edits
the world on one peer only (an agent's `world.set`), a script reading this peer's own facts into the
game (the time of day, `net.info`, a file), or code that depends on something other than the tick
and the inputs. `runtime_tests` (`[net]`) plays the arena as two peers on two threads over TCP: each
presses its own move action at tick 20, both reach tick 240 with the same exposed state and the same
hash chain over the whole run, Red moved by the host's key and Blue by the other's; and a peer that
moves the ball on its own at tick 70 is caught by the host at the next check (tick 89); a third
plays the arena with the other peer joined over a WebSocket (`ws://127.0.0.1:PORT/`), 180 ticks with
the same hashes and no desync.

When a player's connection drops, the host decides the tick from which the game goes on without them
(the one after their last input) and tells everyone, so every peer stops waiting for them at the
same tick (`net.left`); their actions stay at rest from then on.

## Coming back

A running game takes a newcomer into the place of a player who left (`--net-join` as before; the
first such place): a dropped connection comes back, or someone else takes the empty seat. The world
is not sent. The host keeps every input the game has run (the few that were not empty), and the
newcomer gets them all with the absences so far; it loads the scene with the game's seed as everyone
did and replays the game from tick 0 without waiting for anyone, as fast as its ticks run (the
session runs them in slices of 30 ms a frame instead of at the clock's pace), so its world, its
scripts' own state and its random numbers all come out as they are on the others. The hashes it
reports on the way are checked against the ones the host kept of its own world (`replay_checks` in
`net.info`): a replay that went wrong is a desync like any other. Once it has caught up it says so;
the host names a tick far enough ahead for its input to reach everyone (twice the input delay and
ten more past the host's) and tells every peer that the player is back from then (`net.back`, and
the player's absence closes in `net.info.away`). Until that tick nobody waits for it and its keys go
nowhere; from it on, it plays in step. `net.info` says `late`, `catching_up` and `back_at` on the
peer that came back.

The replay costs what the game has run so far, at the speed the peer runs ticks: a long or heavy
game takes longer to come back into (the others do not wait meanwhile, and the history the host
keeps grows with the game). `runtime_tests` (`[rejoin]`) run the arena on a host to tick 900, with a
player who leaves at 150 and another who comes into its place: it replays some 250 ticks, is back
from tick 282, its eight replayed hashes match the host's, its key moves Blue once it is back, and
the two finish at 900 with the same exposed state and the same hash chain over the whole game.

## The wire

Between each player and the host, JSON objects: the host's `welcome` (player number, players, seed,
delay), `start` once everyone is in, `in` (a player's input events for a tick, which the host relays
to the others), `hash`, `desync` and `left`. A tick's input is a few dozen bytes when nothing is
pressed. They go over TCP, one object per line after the player's `hello`, or over a WebSocket, one
object per text frame (RFC 6455: the HTTP upgrade, masked frames from the player, pings answered);
the host tells the two apart by a newcomer's first bytes, so one port takes both and a game can mix
them. `engine/app/src/websocket.cpp` has the handshake's key (SHA-1, base64) and the framing;
`runtime_tests` (`[websocket]`) check them against RFC 6455's own example and frames of 5 to 70000
bytes arriving in pieces.

## Browsers

A browser cannot open a TCP connection, so a web build (`docs/web.md`) joins over a WebSocket: open
the page with `?join=ws://HOST:PORT` and it passes `--net-join` to the runtime, which connects
before the scene is made (a native peer can join the same way with `--net-join ws://HOST:PORT/`).
The host is a native runtime with `--net-host`; a browser cannot listen. The host speaks plain
`ws://`, so the page must be served over `http://` (a page from `https://` may only open `wss://`,
which needs a TLS proxy in front of the host). A browser draws, and so ticks, only while its tab is
shown: a peer in a hidden tab stops and the others wait for it.

Checked: `samples/arena` with a native host and the browser build (`wasm-small`) joined from Chrome,
both players steering, three goals and sixteen collisions in 1392 ticks, no desync
(`tests/evidence/web/arena-join.json` and `.png`).

## Determinism

A lockstep game is one game only if every peer computes the same bits, and a browser and a native
peer are different builds: another compiler's code, another C library, another JavaScript engine.
Three things make them agree.

- **No fused multiply-adds.** An arm64 build would fuse `a * b + c` into one instruction with one
  rounding where WebAssembly rounds twice; `pocket.toml` builds everything with `-ffp-contract=off`,
  so both round twice.
- **Reproducible math.** The platform's `sin`, `cos`, `atan2`, `exp`, `pow` differ in the last bit
  between macOS's libm and the musl in a web build, and between JavaScriptCore (native scripts), V8
  and other engines. `pocket::repro` (`engine/core/include/pocket/core/repro.hpp`) computes them
  from `+ - * /` and `sqrt` alone, in double, rounded to float once, which IEEE 754 fixes to the
  bit; the physics, water, characters, camera rigs, timelines, navigation, particles and
  `Quat::from_axis_angle` use it. Scripts have the same in the SDK: `repro.sin`, `repro.cos`,
  `repro.atan2`, `repro.pow`... (`sdk/runtime/repro.ts`, in doubles, from the same operations),
  which the SDK's tweens and formations use; game logic that runs in lockstep should use them too,
  and `Math.sin` only for what is drawn. Both are about as accurate as the libraries they replace
  (`core_tests` `[repro]`: within an ulp of the double libm rounded to float;
  `tests/ts/repro.test.ts`: within a few ulps of `Math`), and each pins a hash of its results over a
  sweep of inputs: the C++ one holds natively and compiled to WebAssembly under node
  (`python3 tools/scripts/wasm_core_tests.py`), the TypeScript one on JavaScriptCore and on V8.
- **Hashes that know entities by their place.** A component field that holds an entity
  (`Character.ground`, a rigid body's `riding`) is hashed as that entity's place in the world's
  walk, not its id: the ids depend on how many entities the build made before the scene, and a web
  build makes fewer.

With these, the arena run 300 ticks with the same inputs gives the same hash at every tick natively
(debug) and in the browser (`wasm-small`), through the kicks and a goal. Code that reads anything
but the tick and the inputs still drifts, as in any lockstep game (see Staying one game); so does a
script using `Math.sin` for the game on two different JavaScript engines.

## The sample

`samples/arena` is a small pitch with a ball and two goals: Red (player 0) scores in the north goal,
Blue (player 1) in the south; each moves a `Character` with WASD or a pad, and characters push the
ball. Run it with `--net-host` on one machine and `--net-join` on another (or twice on one).

## Not yet

Rollback (predicting the others' input and correcting, instead of waiting for it), more players than
the game started with (a running game takes newcomers only into the places of players who left), a
snapshot of the world to join from instead of the replay (a long game's replay grows with it),
relays through a server for peers that cannot reach each other, a browser hosting (WebRTC between
browsers), `wss://` in the runtime itself, and a hidden tab going on ticking on a timer.

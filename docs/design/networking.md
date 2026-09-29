# Networking

Pocket's multiplayer is lockstep. Every peer runs the whole game from the same scene and the same seed, and a tick runs only once every player's input for that tick has arrived, so every peer computes the same world without sending any of it. The engine was already deterministic per tick (the same inputs make the same state hash, which is what journals, scenarios and replays rely on); lockstep turns that into a network game, and the state hash turns into a check that it stays one game.

## Playing

```bash
pocket run arena -- --net-host 7777                  # player 0, waits for one more
pocket run arena -- --net-join 192.168.1.20:7777     # player 1
```

`--net-host PORT` (0 for any free port) listens for `--net-players` players in all, itself included (2 by default); `--net-join HOST:PORT` connects to a host, takes the player number it is given and the host's seed (before anything is made from it, so the scene, the scripts' `onStart` and every random number are the same on every peer) and the host's input delay (`--net-delay`, 3 ticks). Until every player is in, the peers draw and wait: no tick runs, and those frames do not count against `--frames`. Then all of them start at tick 0.

## A tick

A peer's own input (its keys, pad, mouse, and the synthetic input of `input.hold`, `input.press`, `input.pad`) does not act at once: it waits until the peer is about to run a tick T and goes out for tick T + delay, to everyone through the host. Tick T runs when every player's input for T is in; the first `delay` ticks have none to wait for. Then each player's input goes through that player's own input map (player 0's is the ordinary one), the scripts' `onInput` receives every player's events tagged with `player`, and the tick runs. A peer ahead of the others waits, drawing, until their input arrives; with the delay covering the round trip, nobody waits at all.

Gameplay reads each player's actions by number: `input.axis("move_x", p)`, `input.down("jump", p)`, `input.pressed(...)`, `input.released(...)` (the tick's `players` array holds them all). Without a network game only player 0 has input and every other player's actions are at rest, so a lockstep game plays alone too. `net.info()` (the `net.info` command) says which player this peer is (`player`), whether the game started, the delay, who is connected and who left, and whether the next tick is waiting; use it for the camera and the interface only, since the game's own logic must run the same on every peer.

## Staying one game

Every thirtieth tick each peer reports its world's hash (the world state and the particles) for the tick just run; the host compares them and, when they differ, emits `net.desync` with the tick and every peer's hash into the event log, sends it to everyone, and counts it in `net.info` (`desyncs`, `first_desync`). A desync means something outside the lockstep changed a world: a command that edits the world on one peer only (an agent's `world.set`), a script reading this peer's own facts into the game (the time of day, `net.info`, a file), or code that depends on something other than the tick and the inputs. `runtime_tests` (`[net]`) plays the arena as two peers on two threads over TCP: each presses its own move action at tick 20, both reach tick 240 with the same exposed state and the same hash chain over the whole run, Red moved by the host's key and Blue by the other's; and a peer that moves the ball on its own at tick 70 is caught by the host at the next check (tick 89).

When a player's connection drops, the host decides the tick from which the game goes on without them (the one after their last input) and tells everyone, so every peer stops waiting for them at the same tick (`net.left`); their actions stay at rest from then on.

## The wire

TCP between each player and the host, one JSON object per line: the host's `welcome` (player number, players, seed, delay), `start` once everyone is in, `in` (a player's input events for a tick, which the host relays to the others), `hash`, `desync` and `left`. A tick's input is a few dozen bytes when nothing is pressed.

## The sample

`samples/arena` is a small pitch with a ball and two goals: Red (player 0) scores in the north goal, Blue (player 1) in the south; each moves a `Character` with WASD or a pad, and characters push the ball. Run it with `--net-host` on one machine and `--net-join` on another (or twice on one).

## Not yet

Rollback (predicting the others' input and correcting, instead of waiting for it), players joining a game already running (they would need the world or the whole input history), relays through a server for peers that cannot reach each other, and the browser build (it has no TCP; WebSockets or WebRTC would carry the same messages).

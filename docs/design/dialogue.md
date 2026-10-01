# Dialogue

A conversation is a script: a JSON file (`dialogue/guard.dialogue.json`, or an object in code) of named nodes, each a list of steps. A `Conversation` runs it one line at a time; `dialogue.show` puts it in a box at the bottom of the window. The script is data, so a model writes it the way it writes a scene, `dialogue.check` finds what is wrong with it before anyone plays it, and the game and an agent follow it through events.

```ts
import { dialogue } from "pocket";

const talk = dialogue.start("dialogue/guard.dialogue.json", { gold });   // variables over the script's own
dialogue.show(talk, { onEnd: (c) => { gold = Number(c.vars.gold); } });
```

## The script

```json
{
  "vars": { "gold": 7 },
  "start": "gate",
  "nodes": {
    "gate": [
      { "if": "paid", "then": [ { "say": "Guard", "text": "Already paid. Go on through." }, { "end": true } ] },
      { "say": "Guard", "text": "The toll is five gold." },
      { "choice": [
        { "text": "Pay the toll ({gold} gold left)", "if": "gold >= 5", "set": { "gold": "gold - 5", "paid": true }, "goto": "paid" },
        { "text": "Walk away", "goto": "end" }
      ] }
    ],
    "paid": [
      { "say": "Guard", "text": "Thank you kindly." },
      { "event": "gate.open" }
    ]
  }
}
```

The steps:

- `{say, text}`: a line, `say` naming the speaker (left out for narration). The conversation stops on it until it is moved past.
- `{choice: [...]}`: options, each with `text` and any of `if` (offered only while it holds), `set`, `event` and `goto`. Options whose `if` fails are left out; when none is left the choice is passed over. Without a `goto`, a chosen option goes on with the step after the choice.
- `{set: {name: value}}`: a string is an expression (`"gold - 5"`, `"'Mira'"`), anything else the value itself.
- `{if, then?, else?}`: steps run in place, then on with the node.
- `{goto: "node"}`: on with that node's steps; `"end"` ends the conversation.
- `{event, data?}`: an event emitted when the step is reached, for the game to answer (the gate opens on `gate.open`).
- `{end: true}`: the conversation is over.

A node that runs out ends the conversation (a branch that runs out goes back to the node around it). `start` is the first node, the first in the file when left out.

Expressions are small and safe: numbers, `'strings'`, `true`/`false`, variable names, `+ - * / %` (`+` joins strings), `== != < <= > >=`, `and`/`or`/`not` (or `&& || !`), parentheses. A name not among the variables is undefined, which is false. Anything else (a function call, an unknown character) is an error naming the expression. Text puts in the value of an expression in braces: `"You have {gold} gold"`.

## Running it

`dialogue.start(source, vars?)` makes a `Conversation` from a project file (read through `project.read`) or an object, its variables the script's `vars` with `vars` over them. It runs up to the first line or choice at once. `c.line` is the line now (`{speaker, text}`) or null; `c.choices` the options open now (`{index, text}`, texts filled in); `c.vars` the variables; `c.node` the node it is in; `c.done` whether it is over. `c.next()` moves past a line (nothing while choices wait); `c.choose(i)` takes the `i`-th open option. A loop of gotos with no line in it is stopped after ten thousand steps with an error rather than hanging the game.

Each step tells the event log: `dialogue.line` (speaker, text, node), `dialogue.choice` (index, text, node), `dialogue.end` (node, variables), and a script's own events. They are in the event log and the transcript like any other event, so an agent sees what was said and chosen without looking at the screen.

## The box

`dialogue.show(c, {advance?, speed?, onEnd?})` mounts a Pocket UI box over the bottom of the window: the speaker in gold, the line appearing `speed` letters a second (40; 0 shows it whole), the choices as buttons numbered from 1, with the line they answer kept above them. Space or Enter (or the `advance` action) shows the rest of a line still appearing, then moves past it; a number key or a click takes a choice. The box goes when the conversation is over and `onEnd` is called with it; `show` answers `{close()}` to take it away early. The elements are named (`dialogue`, `dialogue.speaker`, `dialogue.text`, `dialogue.choice.0`, ...), so `ui.click {id: "dialogue.choice.1"}` and `ui.find` reach them as they reach any interface. A game that wants its own box drives a `Conversation` itself and leaves `show` out.

## Checking a script

`dialogue.check(source)` reads a script without running it and lists what is wrong, each problem with its node and step (`"2"`, or `"1.then.0"` inside a branch): a `goto` to a node that is not there, an expression that does not parse (in an `if`, a `set` or the braces of a text), a step it cannot do, a node no `goto` reaches, a `start` that is not among the nodes. An empty list means it is sound. An agent that has just written a conversation runs it before playing; a scenario can hold a project to it (`samples/talk`).

## The sample

`samples/talk`: walk to the guard with the arrows or WASD and press E. The guard asks five gold of the seven you have; asking about the castle loops back to the toll, paying sets the gold and emits `gate.open`, and the game takes the gate away. `pocket scenario talk` plays both answers with the keys a player would press and checks the guard's script.

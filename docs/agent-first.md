# Agent-first principles

"AI agent as a first-class citizen" is a product requirement, not a slogan. The agent is the game maker's developer and the human is the director. The engine's job is to make the agent effective and keep the human in control. Every capability of the engine is therefore available to an agent through stable, machine-readable interfaces, and every claim an agent makes can be verified by running something.

## Principles and the features that implement them

1. **Headless is normal.** The engine renders to offscreen targets without a display, runs a fixed timestep with seeded randomness, and can capture frames, probes and statistics from a script. Golden-image and probe tests are ordinary tests. The agent verifies its own work; the human looks at results, not at test logs.
2. **Everything is a command.** Building, running, testing, capturing frames, querying scenes, editing documents: each is a `pocket` subcommand with `--json` output, a documented schema and meaningful exit codes. The editor and the runtime expose the same operations through MCP, so an agent has the same power as a human at the UI.
3. **One source of truth for the API.** Component metadata declared in C++ generates bindings, `.d.ts` files, inspector panels, serialization and documentation. The types an agent reads are the truth, and documentation cannot drift.
4. **An API models can guess.** Familiar vocabulary, one obvious way per task, doc comments in the declarations. The agent eval suite (see `docs/positioning.md`) measures how well models use the API and treats a drop as a regression.
5. **Text everywhere it matters.** Scenes, prefabs, materials, project settings and metadata are schema-validated text with stable ordering; diffs are reviewable by the human and editable by the agent. Bulk data (textures, meshes, audio) is binary with sidecar metadata.
6. **Deterministic and observable.** Structured logs (JSON lines), per-frame statistics, input recording and replay. An agent can reproduce a bug from a log and prove a fix with a replay.
7. **Explain the change.** The editor shows what the agent did in plain language: files touched, scene edits, settings changed. A non-engineer stays in control of a project they did not type.
8. **Fast loops.** Hot reload of scripts, shaders and assets; incremental builds measured in seconds; tests filterable to one case.
9. **Bounded power.** Scripts run in a sandbox with an explicit capability surface; tools operate within the project directory; secrets never live in project files.

## For engine contributors

Decision records, design documents and evidence directories are the memory both humans and agents work from. A change is not done until its evidence is in the repository.

Definition of done for a change:

- Builds on all configured platforms with warnings as errors.
- Tests added or updated; `pocket test --json` is green.
- Evidence recorded under `tests/evidence/<topic>/` when behavior is visual or numeric.
- Documentation and decision records updated when interfaces or policies change.

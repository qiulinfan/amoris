"""Pocket environment client (docs/design/environment.md): reset, act, observe.

Drives a headless runtime over its JSON-RPC server with nothing but the standard library, so a
bot, a learned player or an evaluation harness can play a project episode by episode:

    from pocket_env import PocketEnv
    with PocketEnv("sprites") as env:
        obs = env.reset(seed=1)
        while not obs["done"]:
            obs = env.step({"move_x": 1, "jump": obs["t"] % 40 == 0}, ticks=4)
        print(obs["score"])

Every call is a runtime command (env.reset, env.step, env.observe, env.describe), so the same
episode can be driven from TypeScript, over MCP, or by hand with curl.
"""
import argparse
import json
import os
import random
import subprocess
import sys
import threading
import urllib.request
from collections import deque
from concurrent.futures import ThreadPoolExecutor


class PocketError(RuntimeError):
    """A command the runtime refused; `code` and `detail` come from the runtime's error."""

    def __init__(self, error):
        data = error.get("data") or {}
        self.code = data.get("code", error.get("code"))
        self.detail = data.get("detail", "")
        super().__init__(f"{self.code}: {error.get('message', '')}")


def find_root(start=None):
    """The repository root: POCKET_ROOT, or the first ancestor of this file with a pocket.toml."""
    if os.environ.get("POCKET_ROOT"):
        return os.environ["POCKET_ROOT"]
    d = os.path.abspath(start or os.path.dirname(__file__))
    while True:
        if os.path.exists(os.path.join(d, "pocket.toml")):
            return d
        parent = os.path.dirname(d)
        if parent == d:
            raise FileNotFoundError("no pocket.toml above " + os.path.dirname(os.path.abspath(__file__)) + "; set POCKET_ROOT")
        d = parent


class PocketEnv:
    """One runtime process playing one project, episode after episode.

    `project` is a project directory, or the name of a sample (`samples/<name>`). The project must
    be bundled (`pocket ts <project>`) and the runtime built (`pocket build`) first.
    """

    def __init__(self, project, *, root=None, config=None, bundle=None, size="320x180", seed=1, max_ticks=0, log_level="warn", runtime=None, extra_args=()):
        self.root = root or find_root()
        config = config or os.environ.get("POCKET_CONFIG", "release")
        project_dir = project if os.path.isabs(project) else os.path.join(self.root, project)
        if not os.path.isdir(project_dir) and os.path.isdir(os.path.join(self.root, "samples", project)):
            project_dir = os.path.join(self.root, "samples", project)
        if not os.path.isdir(project_dir):
            raise FileNotFoundError(f"no project directory at {project_dir}")
        name = os.path.basename(os.path.normpath(project_dir))
        bundle = bundle or os.path.join(self.root, "build", "ts", name + ".js")
        if not os.path.exists(bundle):
            raise FileNotFoundError(f"{bundle} does not exist: bundle the project first (pocket ts {project})")
        runtime = runtime or os.path.join(self.root, "build", config, "bin", "pocket_runtime")
        if not os.path.exists(runtime):
            raise FileNotFoundError(f"{runtime} does not exist: build the runtime first (pocket build)")
        args = [runtime, "--project", project_dir, "--bundle", bundle, "--project-config", bundle + ".project.json",
                "--headless", "--serve", "0", "--paused", "--json", "--size", size, "--seed", str(seed), "--log-level", log_level, *extra_args]
        env = dict(os.environ)
        env.setdefault("POCKET_ROOT", self.root)
        self.process = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, env=env)
        self.url = None
        while self.url is None:
            line = self.process.stderr.readline()
            if not line:
                raise RuntimeError("the runtime exited before it listened")
            if '"listening"' in line:
                self.url = json.loads(line)["url"]
        # The runtime keeps logging to stderr; a full pipe would block it, so a thread drains it
        # and keeps the last lines for error messages (`env.log`).
        self.log = deque(maxlen=200)
        self._drain = threading.Thread(target=self._drain_stderr, daemon=True)
        self._drain.start()
        self._next_id = 0
        self.seed = seed
        self.max_ticks = max_ticks
        self.last = None

    def _drain_stderr(self):
        try:
            for line in self.process.stderr:
                self.log.append(line.rstrip("\n"))
        except (ValueError, OSError):
            pass

    # ---- the interface ----
    def describe(self):
        """The actions the project takes, the keys its observations carry, the score key, the tick rate."""
        return self.command("env.describe")

    def reset(self, seed=None, max_ticks=None):
        """Start an episode: the scene and the scripts start again under the seed. Returns the first observation."""
        if seed is not None:
            self.seed = seed
        if max_ticks is not None:
            self.max_ticks = max_ticks
        self.last = self.command("env.reset", {"seed": self.seed, "max_ticks": self.max_ticks})
        return self.last

    def step(self, actions=None, ticks=1, capture=None):
        """Act, then run `ticks` ticks. `actions` maps action names to a number (held for the step,
        its sign the direction) or True (pressed once). Returns the observation: t, state, score,
        reward (the score's change), done, the events since the last observation."""
        params = {"actions": actions or {}, "ticks": ticks}
        if capture:
            params["capture"] = capture
        self.last = self.command("env.step", params)
        return self.last

    def observe(self, capture=None):
        """The observation now, without running; `capture` writes a PNG of the frame."""
        params = {"capture": capture} if capture else {}
        self.last = self.command("env.observe", params)
        return self.last

    def command(self, method, params=None):
        """Any runtime command (world.query, nav.path, render.ids, ...): the same JSON as over MCP."""
        self._next_id += 1
        body = json.dumps({"id": self._next_id, "method": method, "params": params or {}}).encode()
        req = urllib.request.Request(self.url + "/rpc", data=body, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                reply = json.loads(r.read())
        except OSError as e:   # refused, reset, timed out: say whether the runtime is still there
            code = self.process.poll()
            if code is not None:
                raise RuntimeError(f"the runtime exited with code {code} during {method}; last log lines:\n" + "\n".join(list(self.log)[-10:])) from e
            raise RuntimeError(f"no reply to {method} from the runtime (pid {self.process.pid}): {e}") from e
        if "error" in reply:
            raise PocketError(reply["error"])
        return reply.get("result")

    def close(self):
        if self.process.poll() is None:
            try:
                self.command("quit")
            except Exception:
                pass
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
        if self.process.stderr is not None:
            try:
                self._drain.join(timeout=2)
            except RuntimeError:
                pass
            self.process.stderr.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


class PocketEnvPool:
    """Several runtimes playing the same project side by side, one episode each at a time.

    Every call fans out to the environments in threads (each one blocks on its own HTTP call, the
    runtimes run in parallel) and returns one result per environment, in order.
    """

    def __init__(self, project, n, **kwargs):
        self.envs = [PocketEnv(project, **kwargs) for _ in range(max(1, n))]
        self._pool = ThreadPoolExecutor(max_workers=len(self.envs))

    def __len__(self):
        return len(self.envs)

    def _fan(self, fn, args_per_env):
        return list(self._pool.map(lambda pair: fn(pair[0], *pair[1]), zip(self.envs, args_per_env)))

    def reset(self, seeds=None, max_ticks=None):
        """Start an episode in every environment; `seeds` gives one per environment (the env's own when None)."""
        seeds = list(seeds) if seeds is not None else [None] * len(self.envs)
        return self._fan(lambda env, seed: env.reset(seed=seed, max_ticks=max_ticks), [(s,) for s in seeds])

    def step(self, actions, ticks=1):
        """Act in every environment: `actions` is one action dict per environment (or one dict for all)."""
        if isinstance(actions, dict) or actions is None:
            actions = [actions] * len(self.envs)
        return self._fan(lambda env, act: env.step(act, ticks=ticks), [(a,) for a in actions])

    def observe(self):
        return self._fan(lambda env: env.observe(), [() for _ in self.envs])

    def close(self):
        self._fan(lambda env: env.close(), [() for _ in self.envs])
        self._pool.shutdown()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


def play(project, policy="random", episodes=1, ticks=600, step=4, seed=1, size="320x180"):
    """Play episodes with a scripted policy and return one row per episode."""
    rows = []
    with PocketEnv(project, size=size, seed=seed) as env:
        d = env.describe()
        actions = list(d["actions"].keys())
        for episode in range(episodes):
            rng = random.Random(seed + episode)
            obs = env.reset(seed=seed + episode, max_ticks=ticks)
            steps = 0
            while not obs["done"]:
                if policy == "random":
                    act = {a: rng.choice([-1, 0, 1]) if "negative" in d["actions"][a] else rng.random() < 0.15 for a in actions}
                elif policy == "right":
                    act = {"move_x": 1, "jump": obs["t"] % 40 == 0}
                else:
                    act = {}
                obs = env.step(act, ticks=step)
                steps += 1
            rows.append({"episode": obs["episode"], "seed": seed + episode, "score": obs["score"], "t": obs["t"], "steps": steps, "state": obs["state"]})
    return rows


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description="play a Pocket project through the environment interface")
    ap.add_argument("project", help="a project directory or a sample name")
    ap.add_argument("--policy", default="random", choices=["random", "right", "idle"])
    ap.add_argument("--episodes", type=int, default=1)
    ap.add_argument("--ticks", type=int, default=600, help="ticks per episode")
    ap.add_argument("--step", type=int, default=4, help="ticks per act")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    rows = play(a.project, a.policy, a.episodes, a.ticks, a.step, a.seed)
    if a.json:
        print(json.dumps(rows, indent=2))
    else:
        for r in rows:
            print(f"episode {r['episode']} (seed {r['seed']}): score {r['score']} after {r['t']} ticks, {r['steps']} acts")
    sys.exit(0)

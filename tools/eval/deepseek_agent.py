#!/usr/bin/env python3
"""Bounded DeepSeek tool-agent runner; graders and tool dispatch stay with callers.

No requests occur on import. Credentials never enter artifacts. Thinking history
is retained only in memory and sent back intact; public traces omit reasoning.
Pricing checked 2026-10-05: https://api-docs.deepseek.com/quick_start/pricing/
Thinking/tool protocol: https://api-docs.deepseek.com/guides/thinking_mode/
Usage: https://api-docs.deepseek.com/api/create-chat-completion/
"""

from __future__ import annotations

import contextlib
import copy
import fcntl
import json
import os
import re
import signal
import tempfile
import threading
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import Callable

ENDPOINT = "https://api.deepseek.com/chat/completions"
MODEL = "deepseek-flash"
PRICING_URL = "https://api-docs.deepseek.com/quick_start/pricing/"
PEAK_RATES = {"input_cache_hit": "0.006", "input_cache_miss": "0.30", "output": "1.20"}
MAX_TOTAL_USD = Decimal("5")
CONTEXT_TOKENS = 1_048_576


def utc() -> str:
    return datetime.now(timezone.utc).isoformat()


class BudgetLimit(Exception):
    pass


class TimeLimit(Exception):
    pass


class ProtocolError(Exception):
    pass


@dataclass(frozen=True)
class Limits:
    budget_usd: str = "5"
    max_requests: int = 80
    max_tool_calls: int = 300
    max_seconds: float = 900
    request_timeout_s: float = 120
    max_output_tokens: int = 8192
    max_tool_result_bytes: int = 96_000
    reasoning_effort: str = "low"

    def validate(self) -> None:
        if not Decimal("0") < Decimal(self.budget_usd) <= MAX_TOTAL_USD:
            raise ValueError("This evaluation is authorized for at most $5 total")
        if not 1 <= self.max_output_tokens <= 393_216:
            raise ValueError("max_output_tokens must be within DeepSeek's documented limit")
        if self.reasoning_effort not in {"low", "high", "max", "none"}:
            raise ValueError("Unknown reasoning effort")
        if min(self.max_requests, self.max_tool_calls, self.max_seconds,
               self.request_timeout_s, self.max_tool_result_bytes) <= 0:
            raise ValueError("Limits must be positive")


def money(prompt_hit: int, prompt_miss: int, completion: int) -> Decimal:
    return (Decimal(prompt_hit) * Decimal(PEAK_RATES["input_cache_hit"]) +
            Decimal(prompt_miss) * Decimal(PEAK_RATES["input_cache_miss"]) +
            Decimal(completion) * Decimal(PEAK_RATES["output"])) / Decimal(1_000_000)


def exact_usage(value: object) -> dict:
    if not isinstance(value, dict):
        raise ProtocolError("Provider omitted usage")
    keys = ["prompt_tokens", "completion_tokens", "total_tokens",
            "prompt_cache_hit_tokens", "prompt_cache_miss_tokens"]
    if any(type(value.get(key)) is not int or value[key] < 0 for key in keys):
        raise ProtocolError("Provider usage is incomplete or invalid")
    if value["prompt_tokens"] != value["prompt_cache_hit_tokens"] + value["prompt_cache_miss_tokens"]:
        raise ProtocolError("Provider cache accounting does not sum to prompt_tokens")
    if value["total_tokens"] != value["prompt_tokens"] + value["completion_tokens"]:
        raise ProtocolError("Provider total token accounting does not sum")
    return copy.deepcopy(value)


def public(value: object, secret: str) -> object:
    """Redact auth values and private reasoning even inside returned tool data."""
    if isinstance(value, dict):
        return {key: public(item, secret) for key, item in value.items()
                if str(key).casefold() not in {"reasoning_content", "authorization", "api_key",
                                          "apikey", "access_token", "secret"}}
    if isinstance(value, list):
        return [public(item, secret) for item in value]
    if isinstance(value, str):
        if secret:
            value = value.replace(secret, "[REDACTED]")
        value = re.sub(r"(?i)Bearer\s+[^\s\"']+", "Bearer [REDACTED]", value)
        return re.sub(r"\bsk-[A-Za-z0-9_-]{12,}\b", "[REDACTED]", value)
    return value


def atomic_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


class Ledger:
    """Shared, locked preflight ledger; ambiguous requests keep their reservation."""

    def __init__(self, path: Path | str, budget_usd: str = "5"):
        self.path = Path(path).resolve()
        self.budget = Decimal(budget_usd)
        if not Decimal(0) < self.budget <= MAX_TOTAL_USD:
            raise ValueError("Shared ledger budget must be at most $5")
        self.path.parent.mkdir(parents=True, exist_ok=True)

    @contextlib.contextmanager
    def state(self):
        with self.path.with_suffix(self.path.suffix + ".lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            value = json.loads(self.path.read_text()) if self.path.exists() else {
                "schema_version": 1, "budget_usd": str(self.budget),
                "cost_policy": "peak-rate upper bound; provider billing may be lower",
                "pricing_url": PRICING_URL, "peak_usd_per_million": PEAK_RATES, "requests": [],
            }
            if Decimal(value["budget_usd"]) != self.budget:
                raise ValueError("Existing shared ledger has a different budget; no budget increase allowed")
            yield value
            atomic_json(self.path, value)
            fcntl.flock(lock, fcntl.LOCK_UN)

    @staticmethod
    def committed(state: dict) -> Decimal:
        return sum((Decimal(item.get("cost_upper_usd", item["reserved_usd"]))
                    for item in state["requests"]), Decimal(0))

    def reserve(self, run_id: str, max_output_tokens: int) -> dict:
        # A full documented context is an input upper bound, independent of
        # tokenizer estimates or hidden chat/tool formatting. Assume all misses.
        amount = money(0, CONTEXT_TOKENS, max_output_tokens)
        with self.state() as state:
            if self.committed(state) + amount > Decimal(state["budget_usd"]):
                raise BudgetLimit("Insufficient shared budget for the next bounded request")
            item = {"request_id": str(uuid.uuid4()), "run_id": run_id,
                    "started_at": utc(), "status": "reserved",
                    "reserved_input_tokens": CONTEXT_TOKENS,
                    "reserved_output_tokens": max_output_tokens, "reserved_usd": str(amount)}
            state["requests"].append(item)
        return copy.deepcopy(item)

    def settle(self, request_id: str, usage: dict, model: str) -> dict:
        amount = money(usage["prompt_cache_hit_tokens"], usage["prompt_cache_miss_tokens"],
                       usage["completion_tokens"])
        with self.state() as state:
            item = next(item for item in state["requests"] if item["request_id"] == request_id)
            if item["status"] != "reserved":
                raise ProtocolError("Request was already settled")
            if (usage["prompt_tokens"] > item["reserved_input_tokens"] or
                    usage["completion_tokens"] > item["reserved_output_tokens"] or
                    amount > Decimal(item["reserved_usd"])):
                item["status"] = "reservation_bound_violation"
                item["usage"] = usage
                item["cost_upper_usd"] = str(amount)
                atomic_json(self.path, state)
                raise ProtocolError("Provider exceeded the documented reserved token bounds")
            item.update(status="settled", finished_at=utc(), usage=usage,
                        response_model=model, cost_upper_usd=str(amount))
        return copy.deepcopy(item)

    def ambiguous(self, request_id: str, kind: str) -> None:
        with self.state() as state:
            item = next(item for item in state["requests"] if item["request_id"] == request_id)
            if item["status"] == "reserved":
                item.update(status="unsettled", finished_at=utc(), error_kind=kind)

    def snapshot(self) -> dict:
        with self.state() as state:
            result = copy.deepcopy(state)
            result["committed_upper_usd"] = str(self.committed(state))
            result["remaining_for_reservation_usd"] = str(self.budget - self.committed(state))
        return result


@contextlib.contextmanager
def deadline_guard(seconds: float):
    if threading.current_thread() is not threading.main_thread():
        raise ValueError("run_agent requires the main thread for its wall-time interrupt")
    previous = signal.getsignal(signal.SIGALRM)
    old_timer = signal.getitimer(signal.ITIMER_REAL)
    if old_timer != (0.0, 0.0):
        raise ValueError("run_agent cannot replace an active process alarm")
    def timeout(*_):
        raise TimeLimit()
    signal.signal(signal.SIGALRM, timeout)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def _post(payload: dict, key: str, timeout: float) -> dict:
    request = urllib.request.Request(ENDPOINT, json.dumps(payload, ensure_ascii=False).encode(),
                                     {"Authorization": "Bearer " + key,
                                      "Content-Type": "application/json"})
    # Redirects must never forward the API key to another endpoint.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    opener = urllib.request.build_opener(NoRedirect())
    with opener.open(request, timeout=timeout) as response:
        raw = response.read(32 * 1024 * 1024 + 1)
    if len(raw) > 32 * 1024 * 1024:
        raise ProtocolError("Provider response exceeds the bounded body size")
    return json.loads(raw)


def _key(credential: str | Path | Callable[[], str] | None) -> str:
    if callable(credential):
        key = credential()
    elif isinstance(credential, Path):
        key = credential.read_text().strip()
    else:
        key = credential if credential is not None else os.environ.get("DEEPSEEK_API_KEY", "")
    if not isinstance(key, str) or not key.strip() or any(c in key for c in "\r\n"):
        raise ValueError("Supply a credential in memory, DEEPSEEK_API_KEY, or an external plaintext file")
    return key.strip()


def run_agent(system: str, user: str, tools: list[dict], dispatcher: Callable[[str, dict], object],
              output: Path | str, limits: Limits | dict, credential=None,
              ledger: Ledger | Path | str | None = None) -> dict:
    """Run genuine model-selected tools, returning usage and stop metadata.

    `dispatcher(name, arguments)` must enforce the caller's player/developer
    projection and task authorization. `output` must be a fresh run directory.
    Reuse the same `ledger` path for all trials to share the $5 total budget.
    This runner does not score outcomes or give agents arbitrary shell access.
    """
    limits = Limits(**limits) if isinstance(limits, dict) else limits
    limits.validate()
    if ledger is None:
        raise ValueError("Supply one explicit shared ledger for every gameplay and development trial")
    ledger = ledger if isinstance(ledger, Ledger) else Ledger(Path(ledger), limits.budget_usd)
    if ledger.budget != Decimal(limits.budget_usd):
        raise ValueError("Limits.budget_usd must match the shared total ledger budget")
    key = _key(credential)
    system, user, tools = public(system, key), public(user, key), public(tools, key)
    output = Path(output).resolve()
    if output.exists() and any(output.iterdir()):
        raise ValueError("Use a fresh run output directory")
    output.mkdir(parents=True, exist_ok=True)
    names = [tool["function"]["name"] for tool in tools]
    if len(set(names)) != len(names):
        raise ValueError("Tool names must be unique")
    run_id = str(uuid.uuid4())
    messages = [{"role": "system", "content": system}, {"role": "user", "content": user}]
    started = time.monotonic()
    events = []
    stop = "max_requests"
    tool_calls = tool_errors = 0
    usage_totals = {name: 0 for name in ["prompt_tokens", "completion_tokens", "total_tokens",
                                        "prompt_cache_hit_tokens", "prompt_cache_miss_tokens"]}
    costs = Decimal(0)
    provider_calls = 0
    reservation = None

    def record(event: dict) -> None:
        item = public({"at": utc(), **event}, key)
        events.append(item)
        with (output / "trace.jsonl").open("a") as trace:
            trace.write(json.dumps(item, ensure_ascii=False) + "\n")

    record({"type": "start", "run_id": run_id, "runner": "direct-chat-completions",
            "requested_model": MODEL, "reasoning_effort": limits.reasoning_effort,
            "system": system, "user": user, "tool_names": names})
    try:
        with deadline_guard(limits.max_seconds):
            for step in range(limits.max_requests):
                remaining = limits.max_seconds - (time.monotonic() - started)
                if remaining <= 0:
                    raise TimeLimit()
                payload = {"model": MODEL, "messages": messages,
                           "max_tokens": limits.max_output_tokens, "stream": False,
                           "thinking": {"type": "disabled" if limits.reasoning_effort == "none" else "enabled"}}
                if limits.reasoning_effort != "none":
                    payload["reasoning_effort"] = limits.reasoning_effort
                if tools:
                    payload.update(tools=tools, tool_choice="auto")
                # Byte estimate includes all prior private reasoning; reserve
                # uses the full context rather than trusting this estimate.
                prompt_bytes = len(json.dumps(payload, ensure_ascii=False).encode())
                conservative_text_bound = prompt_bytes * 2 + 4096 + len(messages) * 256
                if conservative_text_bound + limits.max_output_tokens > CONTEXT_TOKENS:
                    stop = "context_bound"
                    break
                reservation = ledger.reserve(run_id, limits.max_output_tokens)
                request_started = time.monotonic()
                record({"type": "request", "step": step + 1,
                        "request_id": reservation["request_id"],
                        "text_token_estimate_upper": conservative_text_bound,
                        "reserved_input_tokens": CONTEXT_TOKENS,
                        "reserved_output_tokens": limits.max_output_tokens,
                        "reserved_usd": reservation["reserved_usd"]})
                provider_calls += 1
                response = _post(payload, key, min(limits.request_timeout_s, remaining))
                usage = exact_usage(response.get("usage"))
                response_model = response.get("model")
                if not isinstance(response_model, str) or not response_model:
                    raise ProtocolError("Provider omitted response model")
                if response_model not in {MODEL, "deepseek-v4-flash", "deepseek-v4-flash-vision-exp",
                                          "DeepSeek-V4.1-Flash"}:
                    raise ProtocolError("Provider response is not a documented Flash alias")
                settled = ledger.settle(reservation["request_id"], usage, response_model)
                reservation = None
                costs += Decimal(settled["cost_upper_usd"])
                for name in usage_totals:
                    usage_totals[name] += usage[name]
                choice = response["choices"][0]
                message = choice["message"]
                if message.get("role") != "assistant":
                    raise ProtocolError("Provider returned an invalid assistant message")
                # Append the whole provider message. In thinking + tools mode,
                # every previous reasoning_content must be passed back.
                messages.append(copy.deepcopy(message))
                record({"type": "response", "step": step + 1,
                        "request_id": settled["request_id"], "provider_id": response.get("id"),
                        "response_model": response_model, "system_fingerprint": response.get("system_fingerprint"),
                        "provider_created": response.get("created"),
                        "latency_s": time.monotonic() - request_started,
                        "usage": usage, "cost_upper_usd": settled["cost_upper_usd"],
                        "finish_reason": choice.get("finish_reason"), "assistant": message})
                calls = message.get("tool_calls") or []
                if not calls:
                    stop = choice.get("finish_reason") or "stop"
                    break
                if choice.get("finish_reason") != "tool_calls":
                    stop = choice.get("finish_reason") or "unexpected_finish_reason"
                    break
                for call in calls:
                    if tool_calls >= limits.max_tool_calls:
                        stop = "max_tool_calls"
                        break
                    tool_calls += 1
                    name = call["function"]["name"]
                    tool_started = time.monotonic()
                    try:
                        arguments = json.loads(call["function"]["arguments"])
                        if not isinstance(arguments, dict):
                            raise ValueError()
                    except (ValueError, TypeError):
                        arguments = None
                        result = {"error": {"code": "invalid_arguments", "message": "Expected a JSON object"}}
                    else:
                        if name not in names:
                            result = {"error": {"code": "unknown_tool", "message": "Tool not authorized"}}
                        else:
                            record({"type": "tool_start", "tool_call_id": call["id"],
                                    "name": name, "arguments": arguments})
                            try:
                                result = dispatcher(name, arguments)
                            except TimeLimit:
                                raise
                            except Exception as error:
                                # Exception bodies can contain provider credentials.
                                result = {"error": {"code": "tool_exception", "type": type(error).__name__}}
                    result = public(result, key)
                    is_error = isinstance(result, dict) and bool(result.get("error") or result.get("isError"))
                    tool_errors += int(is_error)
                    text = result if isinstance(result, str) else json.dumps(result, ensure_ascii=False)
                    if len(text.encode()) > limits.max_tool_result_bytes:
                        result = {"error": {"code": "tool_result_too_large", "bytes": len(text.encode()),
                                            "limit": limits.max_tool_result_bytes}}
                        text = json.dumps(result)
                        if not is_error:
                            tool_errors += 1
                    messages.append({"role": "tool", "tool_call_id": call["id"], "content": text})
                    record({"type": "tool", "tool_call_id": call["id"], "name": name,
                            "arguments": arguments, "result": result,
                            "latency_s": time.monotonic() - tool_started})
                if stop == "max_tool_calls":
                    break
    except BudgetLimit:
        stop = "budget_preflight"
    except TimeLimit:
        stop = "time_limit"
    except urllib.error.HTTPError as error:
        stop = "http_error"
        record({"type": "error", "kind": stop, "status": error.code})
    except Exception as error:
        stop = "protocol_error" if isinstance(error, ProtocolError) else "transport_or_tool_error"
        record({"type": "error", "kind": stop, "exception_type": type(error).__name__})
    finally:
        if reservation is not None:
            ledger.ambiguous(reservation["request_id"], stop)
        shared = ledger.snapshot()
        summary = {"schema_version": 1, "run_id": run_id, "runner": "direct-chat-completions",
                   "requested_model": MODEL, "response_models": sorted({event["response_model"]
                     for event in events if event["type"] == "response"}),
                   "stop_reason": stop, "wall_s": time.monotonic() - started,
                   "provider_calls": provider_calls, "completed_provider_calls": sum(
                       event["type"] == "response" for event in events),
                   "tool_calls": tool_calls, "tool_errors": tool_errors,
                   "usage_totals_known": usage_totals,
                   "metered_cost_upper_usd": str(costs), "billing_cost_usd": None,
                   "cost_policy": "Exact provider usage at peak rates; off-peak/provider bill may be lower",
                   "has_unsettled_request": any(item["run_id"] == run_id and item["status"] != "settled"
                     for item in shared["requests"]),
                   "shared_committed_upper_usd": shared["committed_upper_usd"],
                   "shared_remaining_for_reservation_usd": shared["remaining_for_reservation_usd"],
                   "pricing_url": PRICING_URL, "peak_usd_per_million": PEAK_RATES}
        record({"type": "stop", **summary})
        atomic_json(output / "summary.json", summary)
    return summary

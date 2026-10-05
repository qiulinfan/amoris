"""Offline protocol fixtures only; these are not benchmark runs or API usage."""

import copy
import io
import json
import tempfile
import time
import unittest
import urllib.error
from decimal import Decimal
from pathlib import Path
from unittest.mock import patch

import deepseek_agent as agent

KEY = "sk-offline-fixture-secret-never-published"
TOOLS = [{"type": "function", "function": {"name": "observe", "description": "Fixture",
                                          "parameters": {"type": "object", "properties": {}}}}]


def reply(call=None, reasoning="private fixture reasoning", usage=None):
    message = {"role": "assistant", "content": "done" if call is None else "",
               "reasoning_content": reasoning}
    if call:
        message["tool_calls"] = [{"id": "call-fixture", "type": "function", "function": call}]
    return {"id": "offline-protocol-fixture", "model": "deepseek-flash", "created": 1,
            "choices": [{"message": message, "finish_reason": "tool_calls" if call else "stop"}],
            "usage": usage if usage is not None else {
                "prompt_tokens": 100, "prompt_cache_hit_tokens": 40,
                "prompt_cache_miss_tokens": 60, "completion_tokens": 40,
                "total_tokens": 140, "completion_tokens_details": {"reasoning_tokens": 20}}}


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.ledger = agent.Ledger(self.root / "shared-ledger.json")

    def tearDown(self):
        self.tmp.cleanup()

    def run_fixture(self, responses, dispatcher=lambda *_: {"position": [1, 2, 3]}, **limits):
        with patch.object(agent, "_post", side_effect=responses):
            return agent.run_agent("fixture system", "fixture user", TOOLS, dispatcher,
                                   self.root / "run", agent.Limits(**limits), KEY, self.ledger)

    def test_budget_preflight_persists_and_releases_only_exact_settlement(self):
        ledger = agent.Ledger(self.root / "limited.json", "0.4")
        first = ledger.reserve("fixture-a", 8192)
        with self.assertRaises(agent.BudgetLimit):
            ledger.reserve("fixture-b", 8192)
        usage = agent.exact_usage(reply()["usage"])
        item = ledger.settle(first["request_id"], usage, "deepseek-flash")
        self.assertEqual(Decimal(item["cost_upper_usd"]), Decimal("0.00006624"))
        self.assertEqual(item["reserved_input_tokens"], agent.CONTEXT_TOKENS)
        self.assertEqual(item["reserved_output_tokens"], 8192)
        resumed = agent.Ledger(ledger.path, "0.4")
        resumed.reserve("fixture-b", 8192)
        self.assertLessEqual(Decimal(resumed.snapshot()["committed_upper_usd"]), Decimal("0.4"))

    def test_ledger_cannot_silently_change_budget(self):
        self.ledger.snapshot()
        with self.assertRaises(ValueError):
            agent.Ledger(self.ledger.path, "4").snapshot()
        with self.assertRaises(ValueError):
            agent.Limits(budget_usd="5.01").validate()

    def test_explicit_shared_ledger_is_required_before_loading_credentials(self):
        with patch.object(agent, "_key", side_effect=AssertionError("Must not load a credential")):
            with self.assertRaises(ValueError):
                agent.run_agent("fixture", "fixture", TOOLS, lambda *_: {},
                                self.root / "run", agent.Limits(), ledger=None)

    def test_private_reasoning_is_replayed_but_not_saved(self):
        sent = []
        responses = iter([reply({"name": "observe", "arguments": "{}"}, "PRIVATE-ONE"),
                          reply({"name": "observe", "arguments": "{}"}, "PRIVATE-TWO"), reply()])
        def transport(payload, key, timeout):
            sent.append(copy.deepcopy(payload))
            return next(responses)
        with patch.object(agent, "_post", side_effect=transport):
            result = agent.run_agent("fixture system", "fixture user", TOOLS, lambda *_: {},
                                     self.root / "run", agent.Limits(), KEY, self.ledger)
        self.assertEqual(result["provider_calls"], 3)
        previous = [m for m in sent[2]["messages"] if m["role"] == "assistant"]
        self.assertEqual([m["reasoning_content"] for m in previous], ["PRIVATE-ONE", "PRIVATE-TWO"])
        self.assertEqual([m["tool_call_id"] for m in sent[2]["messages"] if m["role"] == "tool"],
                         ["call-fixture", "call-fixture"])
        trace = (self.root / "run/trace.jsonl").read_text()
        self.assertNotIn("PRIVATE-ONE", trace)
        self.assertNotIn("PRIVATE-TWO", trace)
        self.assertNotIn("reasoning_content", trace)
        self.assertEqual(result["usage_totals_known"]["completion_tokens"], 120)
        self.assertEqual(Decimal(result["metered_cost_upper_usd"]), Decimal("0.00019872"))

    def test_http_error_body_and_credentials_are_never_saved(self):
        error = urllib.error.HTTPError("https://example.test/" + KEY, 401, KEY, {}, io.BytesIO(KEY.encode()))
        result = self.run_fixture([error])
        self.assertEqual(result["stop_reason"], "http_error")
        self.assertTrue(result["has_unsettled_request"])
        self.assertEqual(self.ledger.snapshot()["requests"][0]["status"], "unsettled")
        for file in self.root.rglob("*.json*"):
            self.assertNotIn(KEY, file.read_text())

    def test_tool_redaction_happens_before_model_history(self):
        sent = []
        answers = iter([reply({"name": "observe", "arguments": "{}"}), reply()])
        def transport(payload, *_):
            sent.append(copy.deepcopy(payload))
            return next(answers)
        with patch.object(agent, "_post", side_effect=transport):
            agent.run_agent("fixture", "fixture " + KEY, TOOLS,
                            lambda *_: {"api_key": KEY, "text": "Bearer " + KEY},
                            self.root / "run", agent.Limits(), KEY, self.ledger)
        self.assertNotIn(KEY, json.dumps(sent))
        self.assertNotIn(KEY, (self.root / "run/trace.jsonl").read_text())

    def test_dispatcher_exception_body_is_not_exposed(self):
        def dispatch(*_):
            raise RuntimeError(KEY)
        result = self.run_fixture([reply({"name": "observe", "arguments": "{}"}), reply()], dispatch)
        self.assertEqual(result["tool_errors"], 1)
        self.assertNotIn(KEY, (self.root / "run/trace.jsonl").read_text())

    def test_invalid_or_unknown_tool_never_reaches_dispatcher(self):
        calls = []
        responses = [reply({"name": "observe", "arguments": "not-json"}),
                     reply({"name": "forbidden", "arguments": "{}"}), reply()]
        result = self.run_fixture(responses, lambda *args: calls.append(args))
        self.assertEqual(calls, [])
        self.assertEqual(result["tool_errors"], 2)

    def test_missing_usage_keeps_reservation_and_never_retries(self):
        response = reply()
        response.pop("usage")
        result = self.run_fixture([response])
        self.assertEqual(result["stop_reason"], "protocol_error")
        self.assertEqual(result["provider_calls"], 1)
        self.assertTrue(result["has_unsettled_request"])
        self.assertGreater(Decimal(result["shared_committed_upper_usd"]), Decimal(0))

    def test_request_limit_stops_loop_without_extra_request(self):
        result = self.run_fixture([reply({"name": "observe", "arguments": "{}"})], max_requests=1)
        self.assertEqual(result["provider_calls"], 1)
        self.assertEqual(result["stop_reason"], "max_requests")

    def test_tool_call_batch_obeys_tool_limit(self):
        response = reply({"name": "observe", "arguments": "{}"})
        second = copy.deepcopy(response["choices"][0]["message"]["tool_calls"][0])
        second["id"] = "call-second-fixture"
        response["choices"][0]["message"]["tool_calls"].append(second)
        dispatched = []
        result = self.run_fixture([response], lambda *args: dispatched.append(args), max_tool_calls=1)
        self.assertEqual(len(dispatched), 1)
        self.assertEqual(result["stop_reason"], "max_tool_calls")
        self.assertEqual(result["provider_calls"], 1)

    def test_large_tool_result_is_refused_before_next_prompt(self):
        result = self.run_fixture([reply({"name": "observe", "arguments": "{}"}), reply()],
                                  lambda *_: {"large": "x" * 4096}, max_tool_result_bytes=512)
        self.assertEqual(result["tool_errors"], 1)
        trace = (self.root / "run/trace.jsonl").read_text()
        self.assertIn("tool_result_too_large", trace)
        self.assertNotIn("x" * 4096, trace)

    def test_wall_deadline_interrupts_blocking_request(self):
        def slow(*_):
            time.sleep(0.2)
            return reply()
        with patch.object(agent, "_post", side_effect=slow):
            result = agent.run_agent("fixture", "fixture", TOOLS, lambda *_: {}, self.root / "run",
                                     agent.Limits(max_seconds=0.02), KEY, self.ledger)
        self.assertEqual(result["stop_reason"], "time_limit")
        self.assertLess(result["wall_s"], 0.15)
        self.assertTrue(result["has_unsettled_request"])

    def test_wall_deadline_in_tool_keeps_completed_provider_usage(self):
        def slow_tool(*_):
            time.sleep(0.2)
            return {}
        result = self.run_fixture([reply({"name": "observe", "arguments": "{}"})], slow_tool,
                                  max_seconds=0.02)
        self.assertEqual(result["stop_reason"], "time_limit")
        self.assertEqual(result["usage_totals_known"]["total_tokens"], 140)
        self.assertFalse(result["has_unsettled_request"])

    def test_cache_and_total_usage_inconsistencies_fail_closed(self):
        usage = reply()["usage"]
        usage["prompt_cache_miss_tokens"] += 1
        with self.assertRaises(agent.ProtocolError):
            agent.exact_usage(usage)


if __name__ == "__main__":
    unittest.main()

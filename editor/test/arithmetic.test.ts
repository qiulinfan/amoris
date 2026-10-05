import { expect, test } from "bun:test";
import { evaluateNumber } from "../src/ui/fields/arithmetic";

test("Inspector arithmetic works without a JavaScript evaluator", () => {
  const original = globalThis.Function;
  globalThis.Function = (() => { throw new Error("CSP blocks JS evaluation"); }) as unknown as FunctionConstructor;
  try {
    for (const [input, value] of Object.entries({
      "1": 1, "-0.25": -0.25, ".5": 0.5, "2*3+1": 7, "(2+3)*4": 20,
      "1e-3 + 2E2": 200.001, "2**3**2": 512, "2**-1": 0.5, " 6 / 2 - 1 ": 2,
    })) expect(evaluateNumber(input)).toBe(value);
  } finally { globalThis.Function = original; }
});

test("Inspector rejects code, incomplete expressions, unbounded input, and non-finite values", () => {
  for (const input of ["", "1 2", "1+", "(1", "1)", "1/0", "0/0", "1e999", "NaN",
    "Math.PI", "globalThis", "1;2", "0x10", "alert(1)", "(".repeat(257), "1".repeat(257)]) {
    expect(evaluateNumber(input)).toBeNull();
  }
});

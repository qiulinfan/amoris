#pragma once

// A condition over named numbers, read once and run as often as asked: comparisons, and/or/not
// (or && || !), parentheses, numbers, true and false; a bare name is its value (true when not 0).
// The animation graphs' transitions (docs/design/animation.md, State machines) and the behaviors'
// (docs/design/behavior.md) are written this way.

#include <cstddef>
#include <cstdint>
#include <span>
#include <string>
#include <string_view>
#include <vector>

namespace pocket {

struct ConditionProgram {
    enum class Op : std::uint8_t { Const, Var, Not, Neg, Le, Ge, Eq, Ne, Lt, Gt, And, Or };
    struct Step {
        Op op = Op::Const;
        double value = 0;      // Const
        std::size_t var = 0;   // Var: its index among the names it was read with
    };
    std::vector<Step> steps;
    std::vector<std::size_t> reads;   // the names it reads, by index
    std::string error;                // what could not be read; empty when it reads

    [[nodiscard]] bool empty() const { return steps.empty(); }

    // Its value with `value(i)` the number named by the i-th name; empty programs are 0.
    template <class Value>
    double run(Value&& value) const {
        double stack[64];
        std::size_t top = 0;
        for (const Step& s : steps) {
            switch (s.op) {
                case Op::Const: stack[top++] = s.value; break;
                case Op::Var: stack[top++] = static_cast<double>(value(s.var)); break;
                case Op::Not: stack[top - 1] = stack[top - 1] == 0 ? 1 : 0; break;
                case Op::Neg: stack[top - 1] = -stack[top - 1]; break;
                default: {
                    const double r = stack[--top], l = stack[top - 1];
                    double v = 0;
                    switch (s.op) {
                        case Op::Le: v = l <= r; break;
                        case Op::Ge: v = l >= r; break;
                        case Op::Eq: v = l == r; break;
                        case Op::Ne: v = l != r; break;
                        case Op::Lt: v = l < r; break;
                        case Op::Gt: v = l > r; break;
                        case Op::And: v = (l != 0 && r != 0) ? 1 : 0; break;
                        default: v = (l != 0 || r != 0) ? 1 : 0; break;
                    }
                    stack[top - 1] = v;
                }
            }
        }
        return top > 0 ? stack[top - 1] : 0.0;
    }
};

// Read `text` against `names` (a name not among them is an error, said in the program's `error`).
ConditionProgram read_condition(std::string_view text, std::span<const std::string_view> names);

}  // namespace pocket

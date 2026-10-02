#include <pocket/core/condition.hpp>

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <format>

namespace pocket {

namespace {

// Recursive descent into the program; depth counts the stack the program will need.
class Reader {
   public:
    Reader(std::string_view text, std::span<const std::string_view> names) : s_(text), names_(names) {}
    ConditionProgram read() {
        or_expr();
        skip();
        if (out_.error.empty() && i_ < s_.size()) out_.error = std::format("unexpected '{}'", s_.substr(i_, 12));
        // A program deeper than the evaluator's stack is not one a condition needs.
        if (out_.error.empty() && depth_max_ > 60) out_.error = "too deeply nested";
        return std::move(out_);
    }

   private:
    using Op = ConditionProgram::Op;
    void emit(Op op, double value = 0, std::size_t var = 0) {
        out_.steps.push_back({op, value, var});
        if (op == Op::Const || op == Op::Var) depth_max_ = std::max(depth_max_, ++depth_);
        else if (op != Op::Not && op != Op::Neg) --depth_;
    }
    void fail(std::string why) { if (out_.error.empty()) out_.error = std::move(why); }
    void skip() { while (i_ < s_.size() && std::isspace(static_cast<unsigned char>(s_[i_]))) ++i_; }
    bool take(std::string_view tok) {
        skip();
        if (s_.substr(i_, tok.size()) != tok) return false;
        // A word ends at a word's end: "or" is not the start of "order".
        if (std::isalpha(static_cast<unsigned char>(tok.back())) && i_ + tok.size() < s_.size() && (std::isalnum(static_cast<unsigned char>(s_[i_ + tok.size()])) || s_[i_ + tok.size()] == '_')) return false;
        i_ += tok.size();
        return true;
    }
    void or_expr() {
        and_expr();
        while (take("||") || take("or")) { and_expr(); emit(Op::Or); }
    }
    void and_expr() {
        not_expr();
        while (take("&&") || take("and")) { not_expr(); emit(Op::And); }
    }
    void not_expr() {
        skip();
        if (i_ + 1 < s_.size() && s_[i_] == '!' && s_[i_ + 1] != '=') { ++i_; not_expr(); emit(Op::Not); return; }
        if (take("not")) { not_expr(); emit(Op::Not); return; }
        compare();
    }
    void compare() {
        unary();
        static constexpr std::pair<std::string_view, Op> kOps[] = {{"<=", Op::Le}, {">=", Op::Ge}, {"==", Op::Eq}, {"!=", Op::Ne}, {"<", Op::Lt}, {">", Op::Gt}};
        for (const auto& [tok, op] : kOps) {
            if (!take(tok)) continue;
            unary();
            emit(op);
            return;
        }
    }
    void unary() {
        if (take("-")) { unary(); emit(Op::Neg); return; }
        primary();
    }
    void primary() {
        skip();
        if (take("(")) {
            or_expr();
            if (!take(")")) fail("a '(' without its ')'");
            return;
        }
        if (i_ < s_.size() && (std::isdigit(static_cast<unsigned char>(s_[i_])) || s_[i_] == '.')) {
            const std::string rest(s_.substr(i_));
            char* end = nullptr;
            const double v = std::strtod(rest.c_str(), &end);
            i_ += static_cast<std::size_t>(end - rest.c_str());
            emit(Op::Const, v);
            return;
        }
        const std::size_t start = i_;
        while (i_ < s_.size() && (std::isalnum(static_cast<unsigned char>(s_[i_])) || s_[i_] == '_' || s_[i_] == '.')) ++i_;
        const std::string_view name = s_.substr(start, i_ - start);
        if (name.empty()) {
            fail(i_ < s_.size() ? std::format("unexpected '{}'", s_.substr(i_, 12)) : std::string("it ends too soon"));
            emit(Op::Const, 0);
            return;
        }
        if (name == "true") { emit(Op::Const, 1); return; }
        if (name == "false") { emit(Op::Const, 0); return; }
        for (std::size_t k = 0; k < names_.size(); ++k) {
            if (names_[k] != name) continue;
            out_.reads.push_back(k);
            emit(Op::Var, 0, k);
            return;
        }
        fail(std::format("no parameter '{}'", name));
        emit(Op::Const, 0);
    }
    std::string_view s_;
    std::span<const std::string_view> names_;
    std::size_t i_ = 0;
    int depth_ = 0, depth_max_ = 0;
    ConditionProgram out_;
};

}  // namespace

ConditionProgram read_condition(std::string_view text, std::span<const std::string_view> names) {
    return Reader(text, names).read();
}

}  // namespace pocket

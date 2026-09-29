#include <pocket/ui/syntax.hpp>

#include <algorithm>
#include <array>

namespace pocket::ui {

namespace {

bool ident_start(char c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || c == '$' || static_cast<unsigned char>(c) >= 0x80; }
bool ident_char(char c) { return ident_start(c) || (c >= '0' && c <= '9'); }
bool digit(char c) { return c >= '0' && c <= '9'; }
bool space(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; }

constexpr std::array<std::string_view, 60> kTsKeywords = {
    "abstract", "as", "async", "await", "break", "case", "catch", "class", "const", "continue",
    "debugger", "declare", "default", "delete", "do", "else", "enum", "export", "extends", "false",
    "finally", "for", "from", "function", "get", "if", "implements", "import", "in", "infer",
    "instanceof", "interface", "is", "keyof", "let", "namespace", "new", "null", "of", "private",
    "protected", "public", "readonly", "return", "satisfies", "set", "static", "super", "switch", "this",
    "throw", "true", "try", "type", "typeof", "undefined", "var", "void", "while", "yield",
};

bool ts_keyword(std::string_view w) { return std::find(kTsKeywords.begin(), kTsKeywords.end(), w) != kTsKeywords.end(); }

// A quoted string from `i` (at the quote) to past its closing quote, or to the end of the line
// when it is not closed there (`multiline`: to the end of the text).
std::size_t string_end(std::string_view s, std::size_t i, bool multiline) {
    const char q = s[i];
    for (std::size_t j = i + 1; j < s.size(); ++j) {
        if (s[j] == '\\') { ++j; continue; }
        if (s[j] == q) return j + 1;
        if (s[j] == '\n' && !multiline) return j;
    }
    return s.size();
}

// A number from `i`: digits, letters (hex, suffixes, exponents), dots, underscores, and a sign
// straight after an exponent.
std::size_t number_end(std::string_view s, std::size_t i) {
    std::size_t j = i;
    if (j < s.size() && (s[j] == '-' || s[j] == '+')) ++j;
    const bool hex = j + 1 < s.size() && s[j] == '0' && (s[j + 1] == 'x' || s[j + 1] == 'X');
    while (j < s.size()) {
        const char c = s[j];
        if (ident_char(c) || c == '.') {
            ++j;
            if (!hex && (c == 'e' || c == 'E') && j < s.size() && (s[j] == '+' || s[j] == '-')) ++j;
        } else {
            break;
        }
    }
    return j;
}

void add(std::vector<SyntaxRun>& runs, std::size_t a, std::size_t b, Syntax k) {
    if (b > a) runs.push_back({a, b, k});
}

void lex_ts(std::string_view s, std::vector<SyntaxRun>& runs) {
    std::size_t i = 0;
    char prev = 0;   // the last character that was not a space, for `.name` (a member, not a keyword)
    while (i < s.size()) {
        const char c = s[i];
        if (space(c)) { ++i; continue; }
        if (c == '/' && i + 1 < s.size() && s[i + 1] == '/') {
            std::size_t j = s.find('\n', i);
            if (j == std::string_view::npos) j = s.size();
            add(runs, i, j, Syntax::Comment);
            i = j;
            continue;
        }
        if (c == '/' && i + 1 < s.size() && s[i + 1] == '*') {
            std::size_t j = s.find("*/", i + 2);
            j = j == std::string_view::npos ? s.size() : j + 2;
            add(runs, i, j, Syntax::Comment);
            i = j;
            prev = '/';
            continue;
        }
        if (c == '"' || c == '\'' || c == '`') {
            const std::size_t j = string_end(s, i, c == '`');
            add(runs, i, j, Syntax::String);
            i = j;
            prev = c;
            continue;
        }
        if (digit(c) || (c == '.' && i + 1 < s.size() && digit(s[i + 1]))) {
            const std::size_t j = number_end(s, i);
            add(runs, i, j, Syntax::Number);
            i = j;
            prev = '0';
            continue;
        }
        if (ident_start(c)) {
            std::size_t j = i;
            while (j < s.size() && ident_char(s[j])) ++j;
            const std::string_view w = s.substr(i, j - i);
            std::size_t k = j;
            while (k < s.size() && (s[k] == ' ' || s[k] == '\t')) ++k;
            const bool member = prev == '.';
            if (!member && ts_keyword(w)) add(runs, i, j, Syntax::Keyword);
            else if (k < s.size() && s[k] == '(') add(runs, i, j, Syntax::Function);
            else if (w[0] >= 'A' && w[0] <= 'Z') add(runs, i, j, Syntax::Type);
            i = j;
            prev = 'a';
            continue;
        }
        prev = c;
        ++i;
    }
}

void lex_json(std::string_view s, std::vector<SyntaxRun>& runs) {
    std::size_t i = 0;
    while (i < s.size()) {
        const char c = s[i];
        if (c == '"') {
            const std::size_t j = string_end(s, i, false);
            std::size_t k = j;
            while (k < s.size() && space(s[k])) ++k;
            add(runs, i, j, k < s.size() && s[k] == ':' ? Syntax::Key : Syntax::String);
            i = j;
        } else if (digit(c) || (c == '-' && i + 1 < s.size() && digit(s[i + 1]))) {
            const std::size_t j = number_end(s, i);
            add(runs, i, j, Syntax::Number);
            i = j;
        } else if (ident_start(c)) {
            std::size_t j = i;
            while (j < s.size() && ident_char(s[j])) ++j;
            const std::string_view w = s.substr(i, j - i);
            if (w == "true" || w == "false" || w == "null") add(runs, i, j, Syntax::Keyword);
            i = j;
        } else {
            ++i;
        }
    }
}

void lex_toml(std::string_view s, std::vector<SyntaxRun>& runs) {
    std::size_t i = 0;
    bool line_start = true;   // nothing but spaces since the last newline: a key or a table may begin
    while (i < s.size()) {
        const char c = s[i];
        if (c == '\n') { line_start = true; ++i; continue; }
        if (c == ' ' || c == '\t' || c == '\r') { ++i; continue; }
        if (c == '#') {
            std::size_t j = s.find('\n', i);
            if (j == std::string_view::npos) j = s.size();
            add(runs, i, j, Syntax::Comment);
            i = j;
            continue;
        }
        if (line_start && c == '[') {
            std::size_t j = s.find('\n', i);
            if (j == std::string_view::npos) j = s.size();
            const std::size_t close = s.rfind(']', j);
            const std::size_t end = close != std::string_view::npos && close > i ? close + 1 : j;
            add(runs, i, end, Syntax::Type);
            i = end;
            line_start = false;
            continue;
        }
        if (line_start) {
            // A key up to the `=` (bare, dotted or quoted).
            std::size_t eq = s.find('=', i);
            const std::size_t nl = s.find('\n', i);
            if (eq != std::string_view::npos && (nl == std::string_view::npos || eq < nl)) {
                std::size_t end = eq;
                while (end > i && (s[end - 1] == ' ' || s[end - 1] == '\t')) --end;
                add(runs, i, end, Syntax::Key);
                i = eq + 1;
                line_start = false;
                continue;
            }
        }
        line_start = false;
        if (c == '"' || c == '\'') {
            const bool triple = i + 2 < s.size() && s[i + 1] == c && s[i + 2] == c;
            std::size_t j;
            if (triple) {
                const std::size_t close = s.find(std::string(3, c), i + 3);
                j = close == std::string_view::npos ? s.size() : close + 3;
            } else {
                j = string_end(s, i, false);
            }
            add(runs, i, j, Syntax::String);
            i = j;
        } else if (digit(c) || ((c == '-' || c == '+') && i + 1 < s.size() && digit(s[i + 1]))) {
            // Numbers, and dates and times, which are digits with - : T in them.
            std::size_t j = number_end(s, i);
            while (j < s.size() && (digit(s[j]) || s[j] == '-' || s[j] == ':' || s[j] == 'T' || s[j] == 'Z' || s[j] == '.')) ++j;
            add(runs, i, j, Syntax::Number);
            i = j;
        } else if (ident_start(c)) {
            std::size_t j = i;
            while (j < s.size() && ident_char(s[j])) ++j;
            const std::string_view w = s.substr(i, j - i);
            if (w == "true" || w == "false" || w == "inf" || w == "nan") add(runs, i, j, Syntax::Keyword);
            i = j;
        } else {
            ++i;
        }
    }
}

}  // namespace

std::vector<SyntaxRun> syntax_runs(std::string_view text, std::string_view language) {
    std::vector<SyntaxRun> runs;
    if (language == "ts" || language == "tsx" || language == "js" || language == "typescript" || language == "javascript") lex_ts(text, runs);
    else if (language == "json") lex_json(text, runs);
    else if (language == "toml") lex_toml(text, runs);
    return runs;
}

const char* syntax_name(Syntax kind) {
    switch (kind) {
        case Syntax::Keyword: return "keyword";
        case Syntax::String: return "string";
        case Syntax::Number: return "number";
        case Syntax::Comment: return "comment";
        case Syntax::Type: return "type";
        case Syntax::Function: return "function";
        case Syntax::Key: return "key";
    }
    return "?";
}

std::string_view syntax_for_path(std::string_view path) {
    const std::size_t dot = path.rfind('.');
    if (dot == std::string_view::npos) return {};
    const std::string_view ext = path.substr(dot + 1);
    if (ext == "ts" || ext == "tsx" || ext == "js" || ext == "mjs" || ext == "jsx") return "ts";
    if (ext == "json" || ext == "gltf") return "json";
    if (ext == "toml") return "toml";
    return {};
}

}  // namespace pocket::ui

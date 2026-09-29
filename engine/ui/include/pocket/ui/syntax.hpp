// Syntax colouring for text inputs (the editor's script panel): a text cut into runs of one
// kind, for the languages a project is written in.
#pragma once

#include <cstddef>
#include <cstdint>
#include <string_view>
#include <vector>

namespace pocket::ui {

enum class Syntax : std::uint8_t { Keyword, String, Number, Comment, Type, Function, Key };

struct SyntaxRun {
    std::size_t start = 0, end = 0;   // byte offsets, end exclusive
    Syntax kind = Syntax::Keyword;
};

// The coloured runs of `text` in `language` ("ts" for TypeScript and JavaScript, TSX included;
// "json"; "toml"), in order and apart; what lies between them is plain. An unknown language has
// no runs. The cut is lexical: a keyword is a keyword wherever it stands, a regular expression
// literal is not recognised, and a template string is one string, its ${} parts included.
std::vector<SyntaxRun> syntax_runs(std::string_view text, std::string_view language);

// "keyword", "string", "number", "comment", "type", "function", "key".
const char* syntax_name(Syntax kind);

// The language for a file name by its extension ("main.tsx" -> "ts"), or "" for none.
std::string_view syntax_for_path(std::string_view path);

}  // namespace pocket::ui

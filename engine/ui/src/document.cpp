#include <pocket/ui/document.hpp>

#include <pocket/core/log.hpp>
#include <pocket/ui/syntax.hpp>

#include <yoga/Yoga.h>

#include <algorithm>
#include <array>
#include <cmath>
#include <functional>
#include <map>
#include <optional>
#include <sstream>

namespace pocket::ui {

namespace {

enum Listener : std::uint32_t {
    kClick = 1u << 0, kInput = 1u << 1, kChange = 1u << 2, kKeyDown = 1u << 3, kWheel = 1u << 4,
    kHover = 1u << 5, kFocus = 1u << 6, kDrag = 1u << 7, kMouseDown = 1u << 8, kMouseUp = 1u << 9, kAnimationEnd = 1u << 10,
};

std::uint32_t listener_bit(std::string_view name) {
    if (name == "click") return kClick;
    if (name == "input") return kInput;
    if (name == "change") return kChange;
    if (name == "keydown") return kKeyDown;
    if (name == "wheel") return kWheel;
    if (name == "hover") return kHover;
    if (name == "focus") return kFocus;
    if (name == "drag") return kDrag;
    if (name == "mousedown") return kMouseDown;
    if (name == "mouseup") return kMouseUp;
    if (name == "animationend") return kAnimationEnd;
    return 0;
}

struct Node {
    NodeId id = 0;
    std::string type;  // box, text, input
    std::string name;  // optional, for queries and snapshots
    YGNodeRef yoga = nullptr;
    NodeId parent = 0;
    std::vector<NodeId> children;
    // Paint
    Color background{0, 0, 0, 0};
    std::string image;               // a project-relative picture drawn over the background
    std::string fit = "contain";     // contain (inside, proportions kept), cover (cropped to fill), fill (stretched)
    float uv[4] = {0, 0, 1, 1};      // the part of the picture shown: u0, v0, u1, v1
    float slice[4] = {0, 0, 0, 0};   // nine-slice borders in picture pixels: left, top, right, bottom (all 0: none)
    std::string filter = "linear";   // how the picture is sampled: linear (soft) or nearest (pixel art)
    Color border_color{0, 0, 0, 0};
    float border_width = 0;
    float radius = 0;
    float opacity = 1;
    Color color{0.9f, 0.9f, 0.9f, 1};
    float font_size = 13;
    TextAlign text_align = TextAlign::Left;
    bool text_wrap = false;
    bool clip = false;       // overflow hidden or scroll
    bool scroll = false;     // overflow scroll
    bool visible = true;     // display none => false
    std::uint64_t anchor_entity = 0;     // an entity this element follows on the window (0: none)
    float anchor_offset[2] = {0, 0};     // points added to the projection
    float anchor_align[2] = {0.5f, 1};   // which point of the element sits on the projection (0..1 across, down)
    bool anchor_hidden = false;          // the entity is behind the camera or gone
    // Transitions: a change to a listed prop runs from the present value to the new one over
    // its seconds (eased in and out), advanced by Document::advance.
    std::map<std::string, float> transition;   // prop -> seconds
    struct Anim { float from[4] = {0, 0, 0, 0}, to[4] = {0, 0, 0, 0}; float t = 0, dur = 0; int n = 1; };
    std::map<std::string, Anim> anims;
    // A keyframed animation (`animation`): per prop its keyframes (offset 0..1, value), over
    // `duration` seconds after `delay`, `iterations` times (0: forever), every other one backwards
    // with `alternate`, eased in and out or linear; `source` is the style it came from, so the same
    // style set again (a re-render) does not start it over.
    struct Keyframes {
        std::map<std::string, std::vector<std::pair<float, std::array<float, 4>>>> tracks;
        float duration = 1, delay = 0, t = 0;
        int iterations = 1;
        bool alternate = false, ease = true, finished = false;
        std::string source;
    };
    std::optional<Keyframes> keyframes;
    bool disabled = false;
    std::string text;
    std::string value;       // input
    std::string placeholder;
    int caret = 0;
    int anchor = -1;         // input: the other end of the selection (byte offset), -1 for none
    bool multiline = false;  // input: Return is a new line (with meta or ctrl it commits), Up and Down move by rows
    int last_caret = -1;     // multiline input: the caret at the last paint; a caret that moved is scrolled into view
    std::string syntax;      // multiline input: the language its value is coloured as ("ts", "json", "toml"), or a file name to take it from
    std::vector<SyntaxRun> runs;   // the coloured runs of runs_text, cut at the last paint that needed them
    std::string runs_text, runs_lang;
    std::uint32_t listeners = 0;
    // Layout results (absolute, points)
    Rect rect;
    float scroll_y = 0;
    float content_height = 0;
    // Cached wrapped lines for text nodes
    std::vector<std::string> lines;
    float measured_width = -1;
};

// The colour of each kind of run in a coloured input, for a dark field.
Color syntax_color(Syntax k) {
    switch (k) {
        case Syntax::Keyword: return {0.78f, 0.57f, 0.92f, 1};
        case Syntax::String: return {0.76f, 0.91f, 0.55f, 1};
        case Syntax::Number: return {0.97f, 0.55f, 0.42f, 1};
        case Syntax::Comment: return {0.45f, 0.49f, 0.60f, 1};
        case Syntax::Type: return {1.0f, 0.80f, 0.42f, 1};
        case Syntax::Function: return {0.51f, 0.67f, 1.0f, 1};
        case Syntax::Key: return {0.54f, 0.87f, 1.0f, 1};
    }
    return {1, 1, 1, 1};
}

// An input's language: its `syntax` as given, or the one its file name's extension stands for.
std::string_view language_of(const std::string& syntax) {
    return syntax.find('.') == std::string::npos ? std::string_view(syntax) : syntax_for_path(syntax);
}

Color parse_color(const Json& v, Color fallback) {
    if (v.is_string()) {
        std::string s = v.get<std::string>();
        if (s.empty() || s[0] != '#') return fallback;
        s = s.substr(1);
        auto hex = [&](std::size_t i, std::size_t n) -> float {
            if (i + n > s.size()) return 0;
            return static_cast<float>(std::strtoul(s.substr(i, n).c_str(), nullptr, 16)) / (n == 1 ? 15.0f : 255.0f);
        };
        if (s.size() == 3 || s.size() == 4) return {hex(0, 1), hex(1, 1), hex(2, 1), s.size() == 4 ? hex(3, 1) : 1.0f};
        if (s.size() == 6 || s.size() == 8) return {hex(0, 2), hex(2, 2), hex(4, 2), s.size() == 8 ? hex(6, 2) : 1.0f};
        return fallback;
    }
    if (v.is_array() && v.size() >= 3) {
        return {v[0].get<float>(), v[1].get<float>(), v[2].get<float>(), v.size() > 3 ? v[3].get<float>() : 1.0f};
    }
    if (v.is_null()) return {0, 0, 0, 0};
    return fallback;
}

// A dimension: number => px, "50%" => percent, "auto" => auto.
void apply_dim(const Json& v, void (*px)(YGNodeRef, float), void (*pct)(YGNodeRef, float), void (*autof)(YGNodeRef), YGNodeRef n) {
    if (v.is_number()) px(n, v.get<float>());
    else if (v.is_string()) {
        std::string s = v.get<std::string>();
        if (s == "auto") { if (autof) autof(n); }
        else if (!s.empty() && s.back() == '%') pct(n, std::stof(s.substr(0, s.size() - 1)));
        else if (!s.empty()) px(n, std::stof(s));
    } else if (v.is_null()) {
        if (autof) autof(n);
    }
}

void apply_edge(const Json& v, void (*setter)(YGNodeRef, YGEdge, float), YGNodeRef n) {
    if (v.is_number()) setter(n, YGEdgeAll, v.get<float>());
    else if (v.is_array()) {
        if (v.size() == 2) { setter(n, YGEdgeVertical, v[0].get<float>()); setter(n, YGEdgeHorizontal, v[1].get<float>()); }
        else if (v.size() == 4) { setter(n, YGEdgeTop, v[0].get<float>()); setter(n, YGEdgeRight, v[1].get<float>()); setter(n, YGEdgeBottom, v[2].get<float>()); setter(n, YGEdgeLeft, v[3].get<float>()); }
    } else if (v.is_object()) {
        if (v.contains("top")) setter(n, YGEdgeTop, v["top"].get<float>());
        if (v.contains("right")) setter(n, YGEdgeRight, v["right"].get<float>());
        if (v.contains("bottom")) setter(n, YGEdgeBottom, v["bottom"].get<float>());
        if (v.contains("left")) setter(n, YGEdgeLeft, v["left"].get<float>());
    }
}

std::string color_hex(Color c) {
    char buf[16];
    std::snprintf(buf, sizeof buf, "#%02x%02x%02x%s", static_cast<int>(std::lround(c.r * 255)), static_cast<int>(std::lround(c.g * 255)), static_cast<int>(std::lround(c.b * 255)), c.a < 0.999f ? std::to_string(static_cast<int>(std::lround(c.a * 100))).insert(0, "@").c_str() : "");
    return buf;
}

// The bounds of the line a byte offset is on, for multi-line inputs.
std::size_t line_start_of(const std::string& v, std::size_t at) {
    const std::size_t nl = at == 0 ? std::string::npos : v.rfind('\n', at - 1);
    return nl == std::string::npos ? 0 : nl + 1;
}
std::size_t line_end_of(const std::string& v, std::size_t at) {
    const std::size_t nl = v.find('\n', at);
    return nl == std::string::npos ? v.size() : nl;
}

std::size_t utf8_prev(const std::string& s, std::size_t i) {
    if (i == 0) return 0;
    --i;
    while (i > 0 && (static_cast<unsigned char>(s[i]) & 0xC0) == 0x80) --i;
    return i;
}

std::size_t utf8_next(const std::string& s, std::size_t i) {
    if (i >= s.size()) return s.size();
    ++i;
    while (i < s.size() && (static_cast<unsigned char>(s[i]) & 0xC0) == 0x80) ++i;
    return i;
}

// What a double-click selects around a byte offset: a run of word characters (letters, digits,
// underscores and the accented rest of the Latin range), a run of spaces, a run of punctuation,
// or one CJK character (its own word).
int char_kind(std::uint32_t cp) {
    if (cp == '\n') return 3;
    if (cp >= 0x2E80) return 2;
    if (cp == ' ' || cp == '\t') return 0;
    const bool word = (cp >= '0' && cp <= '9') || (cp >= 'A' && cp <= 'Z') || (cp >= 'a' && cp <= 'z') || cp == '_' || cp >= 0x80;
    return word ? 1 : 4;
}
void word_bounds(const std::string& v, std::size_t at, std::size_t& a, std::size_t& b) {
    a = b = std::min(at, v.size());
    if (v.empty()) return;
    std::size_t i = a, k = i;
    std::uint32_t cp = i < v.size() ? ui::decode_utf8(v, k) : static_cast<std::uint32_t>('\n');
    if (cp == '\n' && i > 0) { i = utf8_prev(v, i); k = i; cp = ui::decode_utf8(v, k); }   // at the end of a line: the character before
    const int kind = char_kind(cp);
    a = i;
    b = k;
    if (kind == 2 || kind == 3) return;
    while (a > 0) { const std::size_t p = utf8_prev(v, a); std::size_t q = p; if (char_kind(ui::decode_utf8(v, q)) != kind) break; a = p; }
    while (b < v.size()) { std::size_t q = b; if (char_kind(ui::decode_utf8(v, q)) != kind) break; b = q; }
}

}  // namespace

struct Document::Impl {
    Font& font;
    std::function<Document::ImageSource(const std::string&)> image_source;
    std::function<bool(std::uint64_t, float&, float&)> anchor_source;
    Rect caret_rect{};   // the focused input's caret at the last paint
    static bool shown(const Node& n) { return n.visible && !n.anchor_hidden; }
    std::function<std::string()> clipboard_get_fn;
    std::function<void(const std::string&)> clipboard_set_fn;
    std::string clipboard_local;   // used when no OS clipboard is wired
    std::string clipboard_get() const { return clipboard_get_fn ? clipboard_get_fn() : clipboard_local; }
    void clipboard_set(const std::string& s) { clipboard_local = s; if (clipboard_set_fn) clipboard_set_fn(s); }
    // The selection of an input as [a, b) byte offsets, when there is one.
    static bool selection_of(const Node& n, std::size_t& a, std::size_t& b) {
        const std::size_t caret = static_cast<std::size_t>(std::clamp(n.caret, 0, static_cast<int>(n.value.size())));
        if (n.anchor < 0) return false;
        const std::size_t anchor = std::min(static_cast<std::size_t>(n.anchor), n.value.size());
        if (anchor == caret) return false;
        a = std::min(anchor, caret);
        b = std::max(anchor, caret);
        return true;
    }
    std::map<NodeId, Node> nodes;
    std::vector<Json> pending;   // events raised between frames (animationend), handed out with the next input
    NodeId root_id = 1;
    YGConfigRef config = nullptr;
    float width = 0, height = 0, scale = 1;
    NodeId hovered = 0, focused = 0, pressed = 0;
    float press_x = 0, press_y = 0, last_x = 0, last_y = 0;
    bool dragging = false;
    std::uint64_t paints = 0;

    explicit Impl(Font& f) : font(f) {
        config = YGConfigNew();
        // React Native style defaults: column direction, no shrinking, relative position, stretch.
        // Content never collapses silently when a panel gets too small; it overflows or scrolls.
        YGConfigSetUseWebDefaults(config, false);
        YGConfigSetPointScaleFactor(config, 1.0f);
        Node& r = nodes[root_id];
        r.id = root_id;
        r.type = "box";
        r.yoga = YGNodeNewWithConfig(config);
        YGNodeStyleSetFlexDirection(r.yoga, YGFlexDirectionColumn);
        YGNodeSetContext(r.yoga, &r);
    }

    ~Impl() {
        for (auto& [id, n] : nodes) {
            if (n.yoga) YGNodeFree(n.yoga);
        }
        if (config) YGConfigFree(config);
    }

    Node* get(NodeId id) {
        auto it = nodes.find(id);
        return it == nodes.end() ? nullptr : &it->second;
    }
    const Node* get(NodeId id) const {
        auto it = nodes.find(id);
        return it == nodes.end() ? nullptr : &it->second;
    }

    // Text measurement for Yoga. Wrapped when the node allows it and a width constraint exists.
    static YGSize measure(YGNodeConstRef yn, float w, YGMeasureMode wm, float h, YGMeasureMode hm) {
        auto* n = static_cast<Node*>(YGNodeGetContext(yn));
        auto* self = static_cast<Impl*>(YGConfigGetContext(YGNodeGetConfig(const_cast<YGNodeRef>(yn))));
        (void)h;
        (void)hm;
        const std::string& text = n->type == "input" ? (n->value.empty() ? n->placeholder : n->value) : n->text;
        float px = n->font_size * self->scale;
        TextMetrics m = self->font.metrics(px);
        float line_h = m.line_height / self->scale;
        float max_w = wm == YGMeasureModeUndefined ? 1e9f : w;
        std::vector<std::string> lines;
        if (n->type == "input" && n->multiline) {
            std::vector<Row> rows;
            self->rows_of(*n, text, n->text_wrap ? max_w : 1e9f, rows);
            for (const Row& row : rows) lines.push_back(text.substr(row.start, row.end - row.start));
        }
        else self->wrap(text, n->font_size, n->text_wrap && n->type == "text" ? max_w : 1e9f, lines);
        float widest = 0;
        for (const auto& l : lines) widest = std::max(widest, self->font.measure(l, px) / self->scale);
        if (lines.empty()) lines.push_back("");
        n->lines = lines;
        n->measured_width = max_w;
        YGSize size;
        size.width = wm == YGMeasureModeExactly ? w : std::min(std::ceil(widest) + (n->type == "input" ? 2.0f : 0.0f), max_w);
        size.height = std::ceil(line_h * static_cast<float>(lines.size())) + (n->type == "input" && n->multiline ? 4.0f : 0.0f);
        return size;
    }

    // A value's lines, split at newlines (the last line may be empty when the value ends with one).
    static void split_lines(const std::string& text, std::vector<std::string>& lines) {
        lines.clear();
        std::size_t start = 0;
        for (;;) {
            const std::size_t nl = text.find('\n', start);
            if (nl == std::string::npos) { lines.push_back(text.substr(start)); break; }
            lines.push_back(text.substr(start, nl - start));
            start = nl + 1;
        }
    }

    // A text area's rows: one per line, or, with textWrap, the lines wrapped at the inner width
    // (Latin words kept whole, CJK per character, a word wider than the area broken where it must
    // be). [start, end) are byte offsets; a hard row ends at a newline or the end of the text.
    struct Row { std::size_t start = 0, end = 0; bool hard = true; };
    void rows_of(const Node& n, const std::string& text, float inner_w, std::vector<Row>& rows) const {
        rows.clear();
        const float px = n.font_size * scale;
        const float max_w = n.text_wrap && inner_w > 2 ? (inner_w - 2) * scale : 1e30f;
        std::size_t start = 0;
        for (;;) {
            const std::size_t nl = text.find('\n', start);
            const std::size_t end = nl == std::string::npos ? text.size() : nl;
            std::size_t row_start = start, i = start;
            float row_w = 0;
            while (i < end) {
                const std::size_t ps = i;
                std::size_t k = i;
                const std::uint32_t cp = decode_utf8(text, k);
                std::size_t pe = k;
                if (cp < 0x2E80 && cp != ' ') {
                    while (pe < end) { std::size_t k2 = pe; const std::uint32_t c2 = decode_utf8(text, k2); if (c2 == ' ' || c2 >= 0x2E80) break; pe = k2; }
                }
                const float pw = font.measure(text.substr(ps, pe - ps), px);
                if (row_w + pw > max_w && row_start < ps && cp != ' ') { rows.push_back({row_start, ps, false}); row_start = ps; row_w = 0; }
                if (pw > max_w && row_start == ps) {
                    // A word wider than the area: broken by characters.
                    std::size_t c = ps;
                    float cw = 0;
                    for (std::size_t j = ps; j < pe;) {
                        const std::size_t jn = utf8_next(text, j);
                        const float w1 = font.measure(text.substr(j, jn - j), px);
                        if (cw + w1 > max_w && c < j) { rows.push_back({c, j, false}); c = j; cw = 0; }
                        cw += w1;
                        j = jn;
                    }
                    row_start = c;
                    row_w = cw;
                    i = pe;
                    continue;
                }
                row_w += pw;
                i = pe;
            }
            rows.push_back({row_start, end, true});
            if (nl == std::string::npos) break;
            start = nl + 1;
        }
    }
    // The row a byte offset is on: the last one starting at or before it (an offset at a soft
    // break belongs to the row after it).
    static std::size_t row_of(const std::vector<Row>& rows, std::size_t at) {
        std::size_t i = 0;
        while (i + 1 < rows.size() && rows[i + 1].start <= at) ++i;
        return i;
    }
    // Where End goes on a row: its end, or before the space a soft break hangs on.
    static std::size_t row_last(const std::string& text, const Row& row) {
        if (!row.hard && row.end > row.start && text[row.end - 1] == ' ') return row.end - 1;
        return row.end;
    }
    // The byte offset on a row nearest an x position (pixels from the row's start).
    std::size_t offset_in_row(const Node& n, const std::string& text, const Row& row, float local_px) const {
        const float px = n.font_size * scale;
        const std::size_t last = row_last(text, row);
        std::size_t best = last, prev = row.start;
        float prev_w = 0;
        for (std::size_t i = row.start; i <= last; i = utf8_next(text, i)) {
            const float w = font.measure(text.substr(row.start, i - row.start), px);
            if (w >= local_px) { best = (i > row.start && w - local_px > local_px - prev_w) ? prev : i; break; }
            prev = i;
            prev_w = w;
            if (i >= last) break;
        }
        return best;
    }
    float inner_width(const Node& n) const {
        return n.rect.w - YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) - YGNodeLayoutGetPadding(n.yoga, YGEdgeRight) - 2 * n.border_width;
    }

    void wrap(const std::string& text, float size_points, float max_width, std::vector<std::string>& lines) const {
        float px = size_points * scale;
        lines.clear();
        std::string current;
        float current_w = 0;
        std::size_t i = 0;
        auto flush = [&]() { lines.push_back(current); current.clear(); current_w = 0; };
        while (i < text.size()) {
            std::size_t start = i;
            std::uint32_t cp = decode_utf8(text, i);
            if (cp == '\n') { flush(); continue; }
            std::string piece;
            // Latin words stay together; CJK breaks per character.
            if (cp < 0x2E80 && cp != ' ') {
                std::size_t j = i;
                while (j < text.size()) {
                    std::size_t k = j;
                    std::uint32_t c2 = decode_utf8(text, k);
                    if (c2 == ' ' || c2 == '\n' || c2 >= 0x2E80) break;
                    j = k;
                }
                piece = text.substr(start, j - start);
                i = j;
            } else {
                piece = text.substr(start, i - start);
            }
            float pw = font.measure(piece, px) / scale;
            if (current_w + pw > max_width && !current.empty()) {
                // Drop a trailing space at the wrap point.
                if (!current.empty() && current.back() == ' ') current.pop_back();
                flush();
                if (piece == " ") continue;
            }
            current += piece;
            current_w += pw;
        }
        lines.push_back(current);
    }

    static float eased(const Node::Anim& a) {
        const float k = a.dur > 0 ? std::clamp(a.t / a.dur, 0.0f, 1.0f) : 1.0f;
        return k * k * (3 - 2 * k);
    }
    // A transitioned prop's value now: the running animation's, else the style's.
    static bool present_value(const Node& n, const std::string& k, float out[4]) {
        if (auto it = n.anims.find(k); it != n.anims.end()) {
            const float e = eased(it->second);
            for (int i = 0; i < 4; ++i) out[i] = it->second.from[i] + (it->second.to[i] - it->second.from[i]) * e;
            return true;
        }
        if (k == "opacity") { out[0] = n.opacity; return true; }
        if (k == "left" || k == "top") { const YGValue v = YGNodeStyleGetPosition(n.yoga, k == "left" ? YGEdgeLeft : YGEdgeTop); if (v.unit != YGUnitPoint) return false; out[0] = v.value; return true; }
        if (k == "width" || k == "height") { const YGValue v = k == "width" ? YGNodeStyleGetWidth(n.yoga) : YGNodeStyleGetHeight(n.yoga); out[0] = v.unit == YGUnitPoint ? v.value : (k == "width" ? n.rect.w : n.rect.h); return true; }
        if (k == "background" || k == "color") { const Color& c = k == "color" ? n.color : n.background; out[0] = c.r; out[1] = c.g; out[2] = c.b; out[3] = c.a; return true; }
        return false;
    }
    static void apply_value(Node& n, const std::string& k, const float v[4]) {
        if (k == "opacity") n.opacity = v[0];
        else if (k == "left") YGNodeStyleSetPosition(n.yoga, YGEdgeLeft, v[0]);
        else if (k == "top") YGNodeStyleSetPosition(n.yoga, YGEdgeTop, v[0]);
        else if (k == "width") YGNodeStyleSetWidth(n.yoga, v[0]);
        else if (k == "height") YGNodeStyleSetHeight(n.yoga, v[0]);
        else if (k == "background") n.background = Color{v[0], v[1], v[2], v[3]};
        else if (k == "color") n.color = Color{v[0], v[1], v[2], v[3]};
    }
    // Take a change to a transitioned prop: it runs from where the prop is now to the new value.
    // False when the value is not one that animates (a percent, "auto"), so the change lands at once.
    static bool start_transition(Node& n, const std::string& k, const Json& v, float seconds) {
        float target[4] = {0, 0, 0, 0}, now[4] = {0, 0, 0, 0};
        int count = 1;
        if (k == "opacity" || k == "left" || k == "top" || k == "width" || k == "height") {
            if (!v.is_number()) return false;
            target[0] = v.get<float>();
        } else if (k == "background" || k == "color") {
            const Color c = parse_color(v, k == "color" ? n.color : n.background);
            target[0] = c.r; target[1] = c.g; target[2] = c.b; target[3] = c.a;
            count = 4;
        } else {
            return false;
        }
        if (!present_value(n, k, now)) return false;
        if (auto it = n.anims.find(k); it != n.anims.end()) {
            bool same = true;
            for (int i = 0; i < count; ++i) same = same && std::fabs(it->second.to[i] - target[i]) < 1e-6f;
            if (same) return true;   // already on its way there
        }
        bool there = true;
        for (int i = 0; i < count; ++i) there = there && std::fabs(now[i] - target[i]) < 1e-6f;
        if (there) { n.anims.erase(k); apply_value(n, k, target); return true; }
        Node::Anim a;
        for (int i = 0; i < 4; ++i) { a.from[i] = now[i]; a.to[i] = target[i]; }
        a.dur = seconds;
        a.n = count;
        n.anims[k] = a;
        apply_value(n, k, now);
        return true;
    }

    // A keyframed animation from its style: {keyframes: [{offset, prop: value, ...}], duration (ms),
    // delay (ms), iterations (a number or "infinite"), direction ("normal" or "alternate"), easing
    // ("ease" or "linear")}; props are those transitions animate. False for something else.
    static bool read_keyframes(Node& n, const Json& v, Node::Keyframes& out) {
        if (!v.is_object() || !v.contains("keyframes") || !v["keyframes"].is_array()) return false;
        const Json& frames = v["keyframes"];
        for (std::size_t i = 0; i < frames.size(); ++i) {
            const Json& f = frames[i];
            if (!f.is_object()) return false;
            const float at = f.contains("offset") && f["offset"].is_number() ? f["offset"].get<float>() : (frames.size() > 1 ? static_cast<float>(i) / static_cast<float>(frames.size() - 1) : 0.0f);
            for (auto& [k, pv] : f.items()) {
                if (k == "offset") continue;
                std::array<float, 4> val{0, 0, 0, 0};
                if (k == "opacity" || k == "left" || k == "top" || k == "width" || k == "height") {
                    if (!pv.is_number()) continue;
                    val[0] = pv.get<float>();
                } else if (k == "background" || k == "color") {
                    const Color c = parse_color(pv, k == "color" ? n.color : n.background);
                    val = {c.r, c.g, c.b, c.a};
                } else {
                    continue;
                }
                out.tracks[k].push_back({std::clamp(at, 0.0f, 1.0f), val});
            }
        }
        for (auto& [k, t] : out.tracks) std::stable_sort(t.begin(), t.end(), [](const auto& a, const auto& b) { return a.first < b.first; });
        out.duration = std::max(v.value("duration", 1000.0f), 1.0f) / 1000.0f;
        out.delay = std::max(v.value("delay", 0.0f), 0.0f) / 1000.0f;
        if (v.contains("iterations") && v["iterations"].is_string()) out.iterations = 0;
        else out.iterations = std::max(static_cast<int>(v.value("iterations", 1.0f)), 1);
        out.alternate = v.value("direction", std::string("normal")) == "alternate";
        out.ease = v.value("easing", std::string("ease")) != "linear";
        out.source = v.dump();
        return !out.tracks.empty();
    }
    // Where a keyframed animation puts its props at its present time.
    static void apply_keyframes(Node& n) {
        Node::Keyframes& a = *n.keyframes;
        const float local = a.t - a.delay;
        float phase = 0;
        if (local > 0) {
            const float cycles = local / a.duration;
            int iteration = static_cast<int>(std::floor(cycles));
            phase = cycles - static_cast<float>(iteration);
            if (a.iterations > 0 && iteration >= a.iterations) { iteration = a.iterations - 1; phase = 1; a.finished = true; }
            if (a.alternate && (iteration % 2) == 1) phase = 1 - phase;
        }
        const float k = a.ease ? phase * phase * (3 - 2 * phase) : phase;
        for (auto& [prop, track] : a.tracks) {
            std::array<float, 4> v = track.front().second;
            if (k >= track.back().first) {
                v = track.back().second;
            } else {
                for (std::size_t i = 1; i < track.size(); ++i) {
                    if (k <= track[i].first) {
                        const float span = std::max(track[i].first - track[i - 1].first, 1e-6f);
                        const float f = std::clamp((k - track[i - 1].first) / span, 0.0f, 1.0f);
                        for (int c = 0; c < 4; ++c) v[static_cast<std::size_t>(c)] = track[i - 1].second[static_cast<std::size_t>(c)] + (track[i].second[static_cast<std::size_t>(c)] - track[i - 1].second[static_cast<std::size_t>(c)]) * f;
                        break;
                    }
                }
            }
            apply_value(n, prop, v.data());
        }
    }

    void apply_style(Node& n, const Json& props) {
        YGNodeRef y = n.yoga;
        for (auto& [k, v] : props.items()) {
            if (k == "animation") {
                // A keyframed animation; the same one set again keeps running where it is.
                if (v.is_null()) { n.keyframes.reset(); continue; }
                if (n.keyframes && n.keyframes->source == v.dump()) continue;
                Node::Keyframes a;
                if (read_keyframes(n, v, a)) { n.keyframes = std::move(a); apply_keyframes(n); }
                continue;
            }
            if (k == "transition") {
                // {prop: milliseconds}: changes to those props animate from then on.
                n.transition.clear();
                if (v.is_object()) for (auto& [pk, pv] : v.items()) if (pv.is_number() && pv.get<float>() > 0) n.transition[pk] = pv.get<float>() / 1000.0f;
                continue;
            }
            if (auto tr = n.transition.find(k); tr != n.transition.end() && start_transition(n, k, v, tr->second)) continue;
            if (k == "width") apply_dim(v, YGNodeStyleSetWidth, YGNodeStyleSetWidthPercent, YGNodeStyleSetWidthAuto, y);
            else if (k == "height") apply_dim(v, YGNodeStyleSetHeight, YGNodeStyleSetHeightPercent, YGNodeStyleSetHeightAuto, y);
            else if (k == "minWidth") apply_dim(v, YGNodeStyleSetMinWidth, YGNodeStyleSetMinWidthPercent, nullptr, y);
            else if (k == "minHeight") apply_dim(v, YGNodeStyleSetMinHeight, YGNodeStyleSetMinHeightPercent, nullptr, y);
            else if (k == "maxWidth") apply_dim(v, YGNodeStyleSetMaxWidth, YGNodeStyleSetMaxWidthPercent, nullptr, y);
            else if (k == "maxHeight") apply_dim(v, YGNodeStyleSetMaxHeight, YGNodeStyleSetMaxHeightPercent, nullptr, y);
            else if (k == "flexBasis") apply_dim(v, YGNodeStyleSetFlexBasis, YGNodeStyleSetFlexBasisPercent, YGNodeStyleSetFlexBasisAuto, y);
            else if (k == "flex") { if (v.is_number()) { YGNodeStyleSetFlexGrow(y, v.get<float>()); YGNodeStyleSetFlexShrink(y, 1); YGNodeStyleSetFlexBasis(y, 0); } }
            else if (k == "flexGrow") YGNodeStyleSetFlexGrow(y, v.get<float>());
            else if (k == "flexShrink") YGNodeStyleSetFlexShrink(y, v.get<float>());
            else if (k == "direction" || k == "flexDirection") {
                std::string s = v.get<std::string>();
                YGNodeStyleSetFlexDirection(y, s == "row" ? YGFlexDirectionRow : s == "row-reverse" ? YGFlexDirectionRowReverse : s == "column-reverse" ? YGFlexDirectionColumnReverse : YGFlexDirectionColumn);
            }
            else if (k == "wrap" || k == "flexWrap") { std::string s = v.is_string() ? v.get<std::string>() : (v.get<bool>() ? "wrap" : "nowrap"); YGNodeStyleSetFlexWrap(y, s == "wrap" ? YGWrapWrap : s == "wrap-reverse" ? YGWrapWrapReverse : YGWrapNoWrap); }
            else if (k == "justify" || k == "justifyContent") {
                std::string s = v.get<std::string>();
                YGNodeStyleSetJustifyContent(y, s == "center" ? YGJustifyCenter : s == "end" || s == "flex-end" ? YGJustifyFlexEnd : s == "space-between" ? YGJustifySpaceBetween : s == "space-around" ? YGJustifySpaceAround : s == "space-evenly" ? YGJustifySpaceEvenly : YGJustifyFlexStart);
            }
            else if (k == "align" || k == "alignItems" || k == "alignSelf") {
                std::string s = v.get<std::string>();
                YGAlign a = s == "center" ? YGAlignCenter : s == "end" || s == "flex-end" ? YGAlignFlexEnd : s == "stretch" ? YGAlignStretch : s == "baseline" ? YGAlignBaseline : s == "auto" ? YGAlignAuto : YGAlignFlexStart;
                if (k == "alignSelf") YGNodeStyleSetAlignSelf(y, a); else YGNodeStyleSetAlignItems(y, a);
            }
            else if (k == "margin") apply_edge(v, YGNodeStyleSetMargin, y);
            else if (k == "padding") apply_edge(v, YGNodeStyleSetPadding, y);
            else if (k == "gap") { if (v.is_number()) YGNodeStyleSetGap(y, YGGutterAll, v.get<float>()); else if (v.is_array() && v.size() == 2) { YGNodeStyleSetGap(y, YGGutterRow, v[0].get<float>()); YGNodeStyleSetGap(y, YGGutterColumn, v[1].get<float>()); } }
            else if (k == "position") { std::string s = v.get<std::string>(); YGNodeStyleSetPositionType(y, s == "absolute" ? YGPositionTypeAbsolute : s == "static" ? YGPositionTypeStatic : YGPositionTypeRelative); }
            else if (k == "left") { if (v.is_number()) YGNodeStyleSetPosition(y, YGEdgeLeft, v.get<float>()); else if (v.is_string()) YGNodeStyleSetPositionPercent(y, YGEdgeLeft, std::stof(v.get<std::string>())); }
            else if (k == "top") { if (v.is_number()) YGNodeStyleSetPosition(y, YGEdgeTop, v.get<float>()); else if (v.is_string()) YGNodeStyleSetPositionPercent(y, YGEdgeTop, std::stof(v.get<std::string>())); }
            else if (k == "right") { if (v.is_number()) YGNodeStyleSetPosition(y, YGEdgeRight, v.get<float>()); }
            else if (k == "bottom") { if (v.is_number()) YGNodeStyleSetPosition(y, YGEdgeBottom, v.get<float>()); }
            else if (k == "overflow") {
                std::string s = v.get<std::string>();
                n.clip = s == "hidden" || s == "scroll";
                n.scroll = s == "scroll";
                YGNodeStyleSetOverflow(y, s == "hidden" ? YGOverflowHidden : s == "scroll" ? YGOverflowScroll : YGOverflowVisible);
            }
            else if (k == "display") { std::string s = v.get<std::string>(); n.visible = s != "none"; YGNodeStyleSetDisplay(y, n.visible && !n.anchor_hidden ? YGDisplayFlex : YGDisplayNone); }
            else if (k == "anchor") {
                n.anchor_entity = v.is_number() ? v.get<std::uint64_t>() : 0;
                if (n.anchor_entity) YGNodeStyleSetPositionType(y, YGPositionTypeAbsolute);
                else if (n.anchor_hidden) { n.anchor_hidden = false; YGNodeStyleSetDisplay(y, n.visible ? YGDisplayFlex : YGDisplayNone); }
            }
            else if (k == "anchorOffset") { if (v.is_array() && v.size() == 2) for (std::size_t i = 0; i < 2; ++i) n.anchor_offset[i] = v[i].is_number() ? v[i].get<float>() : 0.0f; }
            else if (k == "anchorAlign") { if (v.is_array() && v.size() == 2) for (std::size_t i = 0; i < 2; ++i) n.anchor_align[i] = v[i].is_number() ? std::clamp(v[i].get<float>(), 0.0f, 1.0f) : 0.0f; }
            else if (k == "background" || k == "backgroundColor" || k == "bg") n.background = parse_color(v, n.background);
            else if (k == "image") n.image = v.is_string() ? v.get<std::string>() : "";
            else if (k == "fit") n.fit = v.is_string() ? v.get<std::string>() : "contain";
            else if (k == "filter") n.filter = v.is_string() && v.get<std::string>() == "nearest" ? "nearest" : "linear";
            else if (k == "uv") { if (v.is_array() && v.size() == 4) for (std::size_t i = 0; i < 4; ++i) n.uv[i] = v[i].is_number() ? v[i].get<float>() : n.uv[i]; }
            else if (k == "slice") {
                if (v.is_array() && v.size() == 4) for (std::size_t i = 0; i < 4; ++i) n.slice[i] = v[i].is_number() ? std::max(0.0f, v[i].get<float>()) : 0.0f;
                else if (v.is_number()) for (float& e : n.slice) e = std::max(0.0f, v.get<float>());
                else for (float& e : n.slice) e = 0;
            }
            else if (k == "borderColor") n.border_color = parse_color(v, n.border_color);
            else if (k == "border" || k == "borderWidth") { n.border_width = v.is_number() ? v.get<float>() : 0.0f; YGNodeStyleSetBorder(y, YGEdgeAll, n.border_width); }
            else if (k == "radius" || k == "borderRadius") n.radius = v.get<float>();
            else if (k == "opacity") n.opacity = v.get<float>();
            else if (k == "color") n.color = parse_color(v, n.color);
            else if (k == "fontSize") { n.font_size = v.get<float>(); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            else if (k == "textAlign") { std::string s = v.get<std::string>(); n.text_align = s == "center" ? TextAlign::Center : s == "right" ? TextAlign::Right : TextAlign::Left; }
            else if (k == "textWrap" || k == "wrapText") { n.text_wrap = v.get<bool>(); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            else if (k == "text") { n.text = v.is_string() ? v.get<std::string>() : v.dump(); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            else if (k == "value") {
                // The same value again (a re-render echoing what was typed) keeps the caret where it is.
                std::string nv = v.is_string() ? v.get<std::string>() : v.dump();
                if (nv != n.value) { n.value = std::move(nv); n.caret = static_cast<int>(n.value.size()); n.anchor = -1; if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            }
            else if (k == "placeholder") n.placeholder = v.get<std::string>();
            else if (k == "multiline") { n.multiline = v.is_boolean() && v.get<bool>(); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            else if (k == "syntax") n.syntax = v.is_string() ? v.get<std::string>() : std::string();
            else if (k == "name") n.name = v.get<std::string>();
            else if (k == "disabled") n.disabled = v.get<bool>();
            else if (k == "on") {
                n.listeners = 0;
                if (v.is_array()) for (auto& e : v) n.listeners |= listener_bit(e.get<std::string>());
                else if (v.is_string()) n.listeners |= listener_bit(v.get<std::string>());
            }
            else if (k == "scrollTop") { n.scroll_y = v.get<float>(); }
        }
    }

    void compute_rects(Node& n, float ox, float oy) {
        float l = YGNodeLayoutGetLeft(n.yoga), t = YGNodeLayoutGetTop(n.yoga);
        n.rect = {ox + l, oy + t, YGNodeLayoutGetWidth(n.yoga), YGNodeLayoutGetHeight(n.yoga)};
        float content_bottom = 0;
        for (NodeId c : n.children) {
            Node* child = get(c);
            if (!child) continue;
            compute_rects(*child, n.rect.x, n.rect.y - (n.scroll ? n.scroll_y : 0.0f));
            content_bottom = std::max(content_bottom, YGNodeLayoutGetTop(child->yoga) + YGNodeLayoutGetHeight(child->yoga) + YGNodeLayoutGetMargin(child->yoga, YGEdgeBottom));
        }
        n.content_height = content_bottom + YGNodeLayoutGetPadding(n.yoga, YGEdgeBottom);
        if (n.scroll) {
            float max_scroll = std::max(0.0f, n.content_height - n.rect.h);
            if (n.scroll_y > max_scroll || n.scroll_y < 0) {
                n.scroll_y = std::clamp(n.scroll_y, 0.0f, max_scroll);
                for (NodeId c : n.children) if (Node* child = get(c)) compute_rects(*child, n.rect.x, n.rect.y - n.scroll_y);
            }
        }
    }

    void paint_node(Node& n, Painter& p, float opacity) {
        if (!shown(n)) return;
        float op = opacity * n.opacity;
        if (op <= 0) return;
        const Rect& r = n.rect;
        Rect clip = p.current_clip();
        if (r.w <= 0 || r.h <= 0 || clip.intersect(r).w <= 0 || clip.intersect(r).h <= 0) {
            if (!(n.parent == 0)) return;
        }
        if (n.background.a > 0) p.rect(r, n.background.with_alpha(op), n.radius);
        if (!n.image.empty() && image_source) {
            const Document::ImageSource src = image_source(n.image);
            if (src.view && src.width > 0 && src.height > 0) {
                // The picture's box: stretched over the element (fill), fitted inside it with its
                // proportions kept and centered (contain), or covering it with the excess cropped (cover).
                Rect box = r;
                float u0 = n.uv[0], v0 = n.uv[1], u1 = n.uv[2], v1 = n.uv[3];
                const float pw = src.width * (u1 - u0), ph = src.height * (v1 - v0);
                if (n.slice[0] > 0 || n.slice[1] > 0 || n.slice[2] > 0 || n.slice[3] > 0) {
                    // Nine slices fill the box whatever `fit` says: a frame keeps its corners.
                    p.image_sliced(r, src.view, u0, v0, u1, v1, pw, ph, n.slice[0], n.slice[1], n.slice[2], n.slice[3], Color{1, 1, 1, op}, n.filter == "nearest");
                } else if (n.fit == "contain" && pw > 0 && ph > 0) {
                    const float s = std::min(r.w / pw, r.h / ph);
                    box = {r.x + (r.w - pw * s) * 0.5f, r.y + (r.h - ph * s) * 0.5f, pw * s, ph * s};
                } else if (n.fit == "cover" && pw > 0 && ph > 0) {
                    const float s = std::max(r.w / pw, r.h / ph);
                    const float vw = r.w / (s * src.width), vh = r.h / (s * src.height);   // the part that shows, in uv
                    const float cu = (u0 + u1) * 0.5f, cv = (v0 + v1) * 0.5f;
                    u0 = cu - vw * 0.5f; u1 = cu + vw * 0.5f; v0 = cv - vh * 0.5f; v1 = cv + vh * 0.5f;
                }
                if (n.slice[0] <= 0 && n.slice[1] <= 0 && n.slice[2] <= 0 && n.slice[3] <= 0) p.image(box, src.view, u0, v0, u1, v1, Color{1, 1, 1, op}, n.radius, n.filter == "nearest");
            }
        }
        if (n.border_width > 0 && n.border_color.a > 0) p.border(r, n.border_color.with_alpha(op), n.border_width, n.radius);
        // The keyboard's focus ring: a focused element that is not an input (the Tab key got it there).
        if (focused == n.id && n.type != "input" && (n.listeners & kClick)) p.border(r, Color{1, 1, 1, 0.8f * op}, 1.5f, n.radius);
        if (n.type == "text" || n.type == "input") {
            Rect inner{r.x + YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) + n.border_width, r.y + YGNodeLayoutGetPadding(n.yoga, YGEdgeTop) + n.border_width, r.w - YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) - YGNodeLayoutGetPadding(n.yoga, YGEdgeRight) - 2 * n.border_width, r.h - YGNodeLayoutGetPadding(n.yoga, YGEdgeTop) - YGNodeLayoutGetPadding(n.yoga, YGEdgeBottom) - 2 * n.border_width};
            p.push_clip(inner);
            float lh = p.line_height(n.font_size);
            if (n.type == "input" && n.multiline) {
                // Rows from the top (the lines, wrapped when textWrap is set), scrolled by scroll_y
                // (the wheel); a caret that moved is brought into view; the caret on its row.
                const bool placeholder = n.value.empty();
                const Color c = placeholder ? n.color.with_alpha(0.45f * op) : n.color.with_alpha(op);
                const std::string& shown = placeholder ? n.placeholder : n.value;
                std::vector<Row> rows;
                rows_of(n, shown, inner.w, rows);
                const std::size_t caret = static_cast<std::size_t>(std::clamp(n.caret, 0, static_cast<int>(n.value.size())));
                const std::size_t caret_row = placeholder ? 0 : row_of(rows, caret);
                const float view_h = std::max(inner.h - 4, lh);
                n.content_height = lh * static_cast<float>(rows.size()) + 4 + (r.h - inner.h);
                const float max_scroll = std::max(0.0f, n.content_height - r.h);
                if (n.caret != n.last_caret) {
                    const float cy = lh * static_cast<float>(caret_row);
                    if (cy < n.scroll_y) n.scroll_y = cy;
                    if (cy + lh > n.scroll_y + view_h) n.scroll_y = cy + lh - view_h;
                    n.last_caret = n.caret;
                }
                n.scroll_y = std::clamp(n.scroll_y, 0.0f, max_scroll);
                std::size_t sel_a = 0, sel_b = 0;
                const bool selected = !placeholder && selection_of(n, sel_a, sel_b);
                const std::string_view lang = placeholder ? std::string_view() : language_of(n.syntax);
                if (!lang.empty() && (n.runs_text != n.value || n.runs_lang != lang)) {
                    n.runs = syntax_runs(n.value, lang);
                    n.runs_text = n.value;
                    n.runs_lang = std::string(lang);
                }
                for (std::size_t i = 0; i < rows.size(); ++i) {
                    const float ty = inner.y + 2 + lh * static_cast<float>(i) - n.scroll_y;
                    if (ty + lh < inner.y || ty > inner.y + inner.h) continue;
                    const Row& row = rows[i];
                    const std::string line = shown.substr(row.start, row.end - row.start);
                    if (selected && sel_a <= row.end && sel_b > row.start) {
                        // The selected part of this row, with a little past the end when the selection takes the newline.
                        const std::size_t a = std::max(sel_a, row.start), b = std::min(sel_b, row.end);
                        const float x0 = inner.x + 1 + p.measure(line.substr(0, a - row.start), n.font_size);
                        const float x1 = inner.x + 1 + p.measure(line.substr(0, b - row.start), n.font_size) + (sel_b > row.end && row.hard ? 4.0f : 0.0f);
                        p.rect({x0, ty, std::max(x1 - x0, 1.0f), lh}, n.color.with_alpha(0.25f * op));
                    }
                    if (lang.empty()) {
                        p.text(inner.x + 1, ty, line, n.font_size, c);
                        continue;
                    }
                    // The row in pieces: plain text in the input's color, each run in its kind's,
                    // each piece placed where the text before it on the row ends.
                    auto piece = [&](std::size_t a, std::size_t b, Color col) {
                        if (b <= a) return;
                        const float x = inner.x + 1 + p.measure(std::string_view(line).substr(0, a - row.start), n.font_size);
                        p.text(x, ty, std::string_view(line).substr(a - row.start, b - a), n.font_size, col);
                    };
                    auto it = std::lower_bound(n.runs.begin(), n.runs.end(), row.start, [](const SyntaxRun& run, std::size_t at) { return run.end <= at; });
                    std::size_t at = row.start;
                    for (; it != n.runs.end() && it->start < row.end; ++it) {
                        const std::size_t a = std::max(it->start, row.start), b = std::min(it->end, row.end);
                        piece(at, a, c);
                        piece(a, b, syntax_color(it->kind).with_alpha(op));
                        at = b;
                    }
                    piece(at, row.end, c);
                }
                if (focused == n.id) {
                    const Row& row = rows[std::min(caret_row, rows.size() - 1)];
                    const float cx = inner.x + 1 + (placeholder ? 0.0f : p.measure(n.value.substr(row.start, caret - row.start), n.font_size));
                    const float cy = inner.y + 2 + lh * static_cast<float>(caret_row) - n.scroll_y;
                    p.rect({cx, cy + 1, 1, lh - 2}, n.color.with_alpha(op));
                    caret_rect = {cx, cy + 1, 1, lh - 2};
                }
            } else if (n.type == "input") {
                bool placeholder = n.value.empty();
                const std::string& shown = placeholder ? n.placeholder : n.value;
                Color c = placeholder ? n.color.with_alpha(0.45f * op) : n.color.with_alpha(op);
                float ty = inner.y + (inner.h - lh) * 0.5f;
                std::size_t sel_a = 0, sel_b = 0;
                if (!placeholder && selection_of(n, sel_a, sel_b)) {
                    const float x0 = inner.x + 1 + p.measure(n.value.substr(0, sel_a), n.font_size);
                    const float x1 = inner.x + 1 + p.measure(n.value.substr(0, sel_b), n.font_size);
                    p.rect({x0, ty, std::max(x1 - x0, 1.0f), lh}, n.color.with_alpha(0.25f * op));
                }
                p.text(inner.x + 1, ty, shown, n.font_size, c);
                if (focused == n.id) {
                    float cx = inner.x + 1 + p.measure(n.value.substr(0, static_cast<std::size_t>(std::clamp(n.caret, 0, static_cast<int>(n.value.size())))), n.font_size);
                    p.rect({cx, ty + 1, 1, lh - 2}, n.color.with_alpha(op));
                    caret_rect = {cx, ty + 1, 1, lh - 2};
                }
            } else {
                if (n.lines.empty() || (n.text_wrap && std::fabs(n.measured_width - inner.w) > 0.5f)) {
                    wrap(n.text, n.font_size, n.text_wrap ? inner.w : 1e9f, n.lines);
                    n.measured_width = inner.w;
                }
                float total = lh * static_cast<float>(n.lines.size());
                float ty = inner.y + std::max(0.0f, (inner.h - total) * 0.5f);
                for (const auto& line : n.lines) {
                    Rect lr{inner.x, ty, inner.w, lh};
                    p.text_aligned(lr, line, n.font_size, n.color.with_alpha(op), n.text_align, true);
                    ty += lh;
                }
            }
            p.pop_clip();
            if (n.type == "input" && n.multiline && n.content_height > r.h + 0.5f) {
                // A thumb on the right when the rows overflow.
                const float track_h = r.h - 4;
                const float thumb_h = std::max(12.0f, track_h * r.h / n.content_height);
                const float max_scroll = n.content_height - r.h;
                const float thumb_y = r.y + 2 + (track_h - thumb_h) * (max_scroll > 0 ? n.scroll_y / max_scroll : 0);
                p.rect({r.x + r.w - 5, thumb_y, 3, thumb_h}, n.color.with_alpha(0.3f * op), 1.5f);
            }
        }
        if (!n.children.empty()) {
            if (n.clip) p.push_clip(r);
            for (NodeId c : n.children) if (Node* child = get(c)) paint_node(*child, p, op);
            if (n.clip) p.pop_clip();
            if (n.scroll && n.content_height > r.h + 0.5f) {
                // Scrollbar.
                float track_h = r.h - 4;
                float thumb_h = std::max(16.0f, track_h * r.h / n.content_height);
                float max_scroll = n.content_height - r.h;
                float thumb_y = r.y + 2 + (track_h - thumb_h) * (max_scroll > 0 ? n.scroll_y / max_scroll : 0);
                p.rect({r.x + r.w - 6, thumb_y, 4, thumb_h}, Color{1, 1, 1, 0.25f * op}, 2);
            }
        }
    }

    NodeId hit(const Node& n, float x, float y, const Rect& clip) const {
        if (!shown(n)) return 0;
        Rect visible = n.clip ? clip.intersect(n.rect) : clip;
        bool inside = n.rect.contains(x, y) && clip.contains(x, y);
        if (n.clip && !inside) return 0;
        for (auto it = n.children.rbegin(); it != n.children.rend(); ++it) {
            const Node* c = get(*it);
            if (!c) continue;
            NodeId h = hit(*c, x, y, n.clip ? visible : clip);
            if (h) return h;
        }
        return inside ? n.id : 0;
    }

    // Nearest ancestor (including self) with the listener bit.
    NodeId listener_target(NodeId id, std::uint32_t bit) const {
        while (id) {
            const Node* n = get(id);
            if (!n) return 0;
            if (n->listeners & bit) return id;
            id = n->parent;
        }
        return 0;
    }

    void snapshot_node(const Node& n, int depth, const SnapshotOptions& o, int& shown, std::ostringstream& out) const {
        if (shown >= o.max_nodes) return;
        for (int i = 0; i < depth; ++i) out << "  ";
        out << "- " << n.type << "#" << n.id;
        if (!n.name.empty()) out << " " << n.name;
        if (o.layout) out << " [" << std::lround(n.rect.x) << "," << std::lround(n.rect.y) << " " << std::lround(n.rect.w) << "x" << std::lround(n.rect.h) << "]";
        if (n.type == "text") out << " \"" << (n.text.size() > 60 ? n.text.substr(0, 57) + "..." : n.text) << "\"";
        if (n.type == "input") {
            std::string shown = n.value;
            for (std::size_t at = shown.find('\n'); at != std::string::npos; at = shown.find('\n', at + 2)) shown.replace(at, 1, "\\n");
            out << " value=\"" << shown << "\"" << (n.placeholder.empty() ? "" : " placeholder=\"" + n.placeholder + "\"") << (n.multiline ? " multiline" : "");
        }
        if (o.styles && n.background.a > 0) out << " bg=" << color_hex(n.background);
        if (n.listeners) {
            out << " on=";
            bool first = true;
            for (auto [bit, name] : {std::pair{kClick, "click"}, std::pair{kInput, "input"}, std::pair{kChange, "change"}, std::pair{kKeyDown, "keydown"}, std::pair{kWheel, "wheel"}, std::pair{kHover, "hover"}, std::pair{kFocus, "focus"}, std::pair{kDrag, "drag"}, std::pair{kMouseDown, "mousedown"}, std::pair{kMouseUp, "mouseup"}}) {
                if (n.listeners & bit) { out << (first ? "" : ",") << name; first = false; }
            }
        }
        if (focused == n.id) out << " focused";
        if (!Impl::shown(n)) out << " hidden";
        if (n.anchor_entity) out << " anchor=" << n.anchor_entity;
        if (n.scroll || (n.type == "input" && n.multiline && n.content_height > n.rect.h + 0.5f)) out << " scroll=" << std::lround(n.scroll_y) << "/" << std::lround(std::max(0.0f, n.content_height - n.rect.h));
        ++shown;
        bool descend = o.depth < 0 || depth + 1 <= o.depth;
        if (!descend && !n.children.empty()) out << " (" << n.children.size() << " children)";
        out << "\n";
        if (!descend) return;
        for (std::size_t i = 0; i < n.children.size(); ++i) {
            if (shown >= o.max_nodes) {
                for (int d = 0; d < depth + 1; ++d) out << "  ";
                out << "  (+" << (n.children.size() - i) << " more)\n";
                return;
            }
            if (const Node* c = get(n.children[i])) snapshot_node(*c, depth + 1, o, shown, out);
        }
    }
};

Document::Document(Font& font) : impl_(std::make_unique<Impl>(font)) {
    YGConfigSetContext(impl_->config, impl_.get());
}
Document::~Document() = default;

void Document::set_clipboard(std::function<std::string()> get, std::function<void(const std::string&)> set) {
    impl_->clipboard_get_fn = std::move(get);
    impl_->clipboard_set_fn = std::move(set);
}

void Document::set_image_source(std::function<ImageSource(const std::string&)> source) {
    impl_->image_source = std::move(source);
}

NodeId Document::root() const { return impl_->root_id; }
bool Document::exists(NodeId id) const { return impl_->get(id) != nullptr; }
std::size_t Document::node_count() const { return impl_->nodes.size(); }
NodeId Document::focused() const { return impl_->focused; }

Result<NodeId> Document::create(NodeId id, std::string_view type) {
    if (id == 0) return fail("bad_args", "node ids start at 1");
    if (impl_->get(id)) return fail("ui_duplicate", "node {} already exists", id);
    if (type != "box" && type != "text" && type != "input") return fail("bad_args", "unknown element type '{}'", type);
    Node& n = impl_->nodes[id];
    n.id = id;
    n.type = std::string(type);
    n.yoga = YGNodeNewWithConfig(impl_->config);
    YGNodeStyleSetFlexDirection(n.yoga, YGFlexDirectionColumn);
    YGNodeSetContext(n.yoga, &n);
    if (type == "text" || type == "input") {
        YGNodeSetNodeType(n.yoga, YGNodeTypeText);
        YGNodeSetMeasureFunc(n.yoga, &Impl::measure);
        if (type == "input") {
            n.background = Color{1, 1, 1, 0.06f};
            n.border_color = Color{1, 1, 1, 0.18f};
            n.border_width = 1;
            n.radius = 4;
            YGNodeStyleSetBorder(n.yoga, YGEdgeAll, 1);
            YGNodeStyleSetPadding(n.yoga, YGEdgeHorizontal, 6);
            YGNodeStyleSetPadding(n.yoga, YGEdgeVertical, 3);
            YGNodeStyleSetMinWidth(n.yoga, 40);
        }
    }
    return id;
}

Status Document::set(NodeId id, const Json& props) {
    Node* n = impl_->get(id);
    if (!n) return fail("ui_no_such_node", "node {} does not exist", id);
    if (!props.is_object()) return fail("bad_args", "props must be an object");
    impl_->apply_style(*n, props);
    return {};
}

Status Document::append(NodeId parent, NodeId child, int index) {
    Node* p = impl_->get(parent);
    Node* c = impl_->get(child);
    if (!p || !c) return fail("ui_no_such_node", "append {} -> {}: missing node", child, parent);
    if (child == impl_->root_id) return fail("bad_args", "the root cannot be moved");
    for (NodeId a = parent; a;) {
        if (a == child) return fail("bad_args", "cannot append a node into its own subtree");
        Node* an = impl_->get(a);
        a = an ? an->parent : 0;
    }
    if (c->parent) {
        Node* old = impl_->get(c->parent);
        if (old) {
            old->children.erase(std::remove(old->children.begin(), old->children.end(), child), old->children.end());
            YGNodeRemoveChild(old->yoga, c->yoga);
        }
    }
    std::size_t at = index < 0 || static_cast<std::size_t>(index) > p->children.size() ? p->children.size() : static_cast<std::size_t>(index);
    p->children.insert(p->children.begin() + static_cast<std::ptrdiff_t>(at), child);
    YGNodeInsertChild(p->yoga, c->yoga, at);
    c->parent = parent;
    return {};
}

Status Document::remove(NodeId id) {
    Node* n = impl_->get(id);
    if (!n) return fail("ui_no_such_node", "node {} does not exist", id);
    if (id == impl_->root_id) return fail("bad_args", "the root cannot be removed");
    std::vector<NodeId> subtree;
    std::function<void(NodeId)> collect = [&](NodeId x) {
        subtree.push_back(x);
        if (Node* xn = impl_->get(x)) for (NodeId c : xn->children) collect(c);
    };
    collect(id);
    if (n->parent) {
        Node* p = impl_->get(n->parent);
        if (p) {
            p->children.erase(std::remove(p->children.begin(), p->children.end(), id), p->children.end());
            YGNodeRemoveChild(p->yoga, n->yoga);
        }
    }
    for (NodeId x : subtree) {
        if (impl_->focused == x) impl_->focused = 0;
        if (impl_->hovered == x) impl_->hovered = 0;
        if (impl_->pressed == x) impl_->pressed = 0;
        Node* xn = impl_->get(x);
        if (xn && xn->yoga) YGNodeFree(xn->yoga);
        impl_->nodes.erase(x);
    }
    return {};
}

Status Document::set_text(NodeId id, std::string_view text) {
    Node* n = impl_->get(id);
    if (!n) return fail("ui_no_such_node", "node {} does not exist", id);
    if (n->type == "input") n->value = std::string(text);
    else n->text = std::string(text);
    n->lines.clear();
    if (YGNodeHasMeasureFunc(n->yoga)) YGNodeMarkDirty(n->yoga);
    return {};
}

Status Document::apply(const Json& ops) {
    if (!ops.is_array()) return fail("bad_args", "ops must be an array");
    for (const Json& op : ops) {
        if (!op.is_array() || op.empty() || !op[0].is_string()) return fail("bad_args", "each op is [name, ...]");
        std::string name = op[0].get<std::string>();
        auto id_at = [&](std::size_t i) -> NodeId { return op.size() > i && op[i].is_number() ? op[i].get<NodeId>() : 0; };
        if (name == "create") { POCKET_TRY_VOID(create(id_at(1), op.size() > 2 && op[2].is_string() ? op[2].get<std::string>() : "box")); }
        else if (name == "set") { POCKET_TRY_VOID(set(id_at(1), op.size() > 2 ? op[2] : Json::object())); }
        else if (name == "append") { POCKET_TRY_VOID(append(id_at(1), id_at(2), op.size() > 3 && op[3].is_number() ? op[3].get<int>() : -1)); }
        else if (name == "remove") { POCKET_TRY_VOID(remove(id_at(1))); }
        else if (name == "text") { POCKET_TRY_VOID(set_text(id_at(1), op.size() > 2 && op[2].is_string() ? op[2].get<std::string>() : "")); }
        else if (name == "clear") {
            Node* n = impl_->get(id_at(1));
            if (!n) return fail("ui_no_such_node", "node {} does not exist", id_at(1));
            std::vector<NodeId> kids = n->children;
            for (NodeId c : kids) POCKET_TRY_VOID(remove(c));
        }
        else if (name == "focus") { set_focus(id_at(1)); }
        else return fail("bad_args", "unknown ui op '{}'", name);
    }
    return {};
}

void Document::layout(float width, float height, float scale) {
    Impl& im = *impl_;
    im.width = width;
    im.height = height;
    if (im.scale != scale) {
        im.scale = scale;
        for (auto& [id, n] : im.nodes) if (YGNodeHasMeasureFunc(n.yoga)) YGNodeMarkDirty(n.yoga);
    }
    Node& root = im.nodes[im.root_id];
    YGNodeStyleSetWidth(root.yoga, width);
    YGNodeStyleSetHeight(root.yoga, height);
    // Anchored elements sit on their entity's projection, the alignment point of their own box
    // (from the last layout) over it; a second pass corrects the box whose size the first changed.
    auto place = [&]() -> bool {
        bool moved = false;
        for (auto& [id, n] : im.nodes) {
            if (!n.anchor_entity) continue;
            float x = 0, y = 0;
            const bool there = im.anchor_source && im.anchor_source(n.anchor_entity, x, y);
            if (there == n.anchor_hidden) {
                n.anchor_hidden = !there;
                YGNodeStyleSetDisplay(n.yoga, there && n.visible ? YGDisplayFlex : YGDisplayNone);
                moved = true;
            }
            if (!there) continue;
            const Node* parent = im.get(n.parent);
            const float left = x + n.anchor_offset[0] - n.rect.w * n.anchor_align[0] - (parent ? parent->rect.x : 0.0f);
            const float top = y + n.anchor_offset[1] - n.rect.h * n.anchor_align[1] - (parent ? parent->rect.y : 0.0f);
            const YGValue cl = YGNodeStyleGetPosition(n.yoga, YGEdgeLeft), ct = YGNodeStyleGetPosition(n.yoga, YGEdgeTop);
            if (cl.unit != YGUnitPoint || ct.unit != YGUnitPoint || std::fabs(cl.value - left) > 0.01f || std::fabs(ct.value - top) > 0.01f) {
                YGNodeStyleSetPosition(n.yoga, YGEdgeLeft, left);
                YGNodeStyleSetPosition(n.yoga, YGEdgeTop, top);
                moved = true;
            }
        }
        return moved;
    };
    const bool anchored = place();
    YGNodeCalculateLayout(root.yoga, width, height, YGDirectionLTR);
    im.compute_rects(root, 0, 0);
    if (anchored && place()) {
        YGNodeCalculateLayout(root.yoga, width, height, YGDirectionLTR);
        im.compute_rects(root, 0, 0);
    }
}

void Document::set_anchor_source(std::function<bool(std::uint64_t, float&, float&)> source) { impl_->anchor_source = std::move(source); }

void Document::advance(float seconds) {
    if (seconds <= 0) return;
    for (auto& [id, n] : impl_->nodes) {
        if (n.keyframes && !n.keyframes->finished) {
            n.keyframes->t += seconds;
            Impl::apply_keyframes(n);
            if (n.keyframes->finished && (n.listeners & kAnimationEnd)) {
                Json e{{"type", "animationend"}, {"id", id}};
                if (!n.name.empty()) e["name"] = n.name;
                impl_->pending.push_back(std::move(e));
            }
        }
        if (n.anims.empty()) continue;
        std::vector<std::string> done;
        for (auto& [k, a] : n.anims) {
            a.t += seconds;
            float v[4];
            const float e = Impl::eased(a);
            for (int i = 0; i < 4; ++i) v[i] = a.from[i] + (a.to[i] - a.from[i]) * e;
            Impl::apply_value(n, k, v);
            if (a.t >= a.dur) done.push_back(k);
        }
        for (const std::string& k : done) n.anims.erase(k);
    }
}

void Document::paint(Painter& painter) {
    Impl& im = *impl_;
    Node& root = im.nodes[im.root_id];
    im.paint_node(root, painter, 1.0f);
    im.paints++;
}

NodeId Document::hit_test(float x, float y) const {
    const Node* root = impl_->get(impl_->root_id);
    if (!root) return 0;
    NodeId h = impl_->hit(*root, x, y, Rect{0, 0, impl_->width, impl_->height});
    return h == impl_->root_id ? 0 : h;
}

void Document::set_focus(NodeId id) {
    if (id != 0 && !impl_->get(id)) return;
    impl_->focused = id;
}

std::vector<Json> Document::handle_events(const std::vector<platform::Event>& events, bool& text_input_wanted) {
    Impl& im = *impl_;
    std::vector<Json> out = std::move(im.pending);
    im.pending.clear();
    auto emit = [&](NodeId target, const char* type, Json extra = Json::object()) {
        Json e = std::move(extra);
        e["type"] = type;
        e["id"] = target;
        if (const Node* n = im.get(target); n && !n->name.empty()) e["name"] = n->name;
        out.push_back(std::move(e));
    };
    auto with_mods = [](Json j, int mods) {
        if (mods) j["mods"] = platform::mods_to_json(mods);
        return j;
    };
    // The byte offset in an input nearest a point: in a text area, on the row under it.
    auto caret_at = [&](const Node& n, float x, float y) -> std::size_t {
        const float local = (x - n.rect.x - YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) - n.border_width - 1) * im.scale;
        if (!n.multiline) return im.offset_in_row(n, n.value, Impl::Row{0, n.value.size(), true}, local);
        const float lh = im.font.metrics(n.font_size * im.scale).line_height / im.scale;
        const float top = n.rect.y + YGNodeLayoutGetPadding(n.yoga, YGEdgeTop) + n.border_width + 2;
        std::vector<Impl::Row> rows;
        im.rows_of(n, n.value, im.inner_width(n), rows);
        const int line = static_cast<int>(std::floor((y - top + n.scroll_y) / std::max(lh, 1.0f)));
        const std::size_t ri = static_cast<std::size_t>(std::clamp(line, 0, static_cast<int>(rows.size()) - 1));
        return im.offset_in_row(n, n.value, rows[ri], local);
    };
    auto set_focus_to = [&](NodeId id) {
        if (im.focused == id) return;
        if (im.focused) {
            NodeId t = im.listener_target(im.focused, kFocus);
            if (t) emit(t, "blur");
            if (Node* old = im.get(im.focused); old && old->type == "input") {
                NodeId ct = im.listener_target(im.focused, kChange);
                if (ct) emit(ct, "change", Json{{"value", old->value}});
            }
        }
        im.focused = id;
        if (id) {
            NodeId t = im.listener_target(id, kFocus);
            if (t) emit(t, "focus");
        }
    };
    for (const platform::Event& ev : events) {
        using platform::EventType;
        switch (ev.type) {
            case EventType::MouseMove: {
                im.last_x = ev.x;
                im.last_y = ev.y;
                NodeId h = hit_test(ev.x, ev.y);
                if (h != im.hovered) {
                    NodeId old_t = im.listener_target(im.hovered, kHover);
                    NodeId new_t = im.listener_target(h, kHover);
                    if (old_t && old_t != new_t) emit(old_t, "hover", Json{{"entered", false}});
                    if (new_t && old_t != new_t) emit(new_t, "hover", Json{{"entered", true}});
                    im.hovered = h;
                }
                if (im.pressed) {
                    if (Node* pn = im.get(im.pressed); pn && pn->type == "input" && !pn->disabled && im.focused == im.pressed) {
                        // Dragging inside an input selects from where the press put the caret.
                        if (pn->anchor < 0) pn->anchor = pn->caret;
                        pn->caret = static_cast<int>(caret_at(*pn, ev.x, ev.y));
                    }
                    NodeId dt = im.listener_target(im.pressed, kDrag);
                    if (dt) {
                        im.dragging = true;
                        emit(dt, "drag", Json{{"x", ev.x}, {"y", ev.y}, {"dx", ev.dx}, {"dy", ev.dy}, {"startX", im.press_x}, {"startY", im.press_y}});
                    }
                }
                break;
            }
            case EventType::MouseDown: {
                NodeId h = hit_test(ev.x, ev.y);
                im.pressed = h;
                im.press_x = ev.x;
                im.press_y = ev.y;
                im.dragging = false;
                Node* n = im.get(h);
                if (n && n->type == "input" && !n->disabled) {
                    const bool was_focused = im.focused == h;
                    set_focus_to(h);
                    // The caret goes to the boundary nearest the click (in a text area, on the line under
                    // it); with Shift the selection extends from where it was.
                    const bool shift = (ev.mods & platform::kModShift) != 0;
                    if (shift && was_focused) { if (n->anchor < 0) n->anchor = n->caret; }
                    else n->anchor = -1;
                    n->caret = static_cast<int>(caret_at(*n, ev.x, ev.y));
                    if (ev.clicks >= 2) {
                        // A double-click selects the word under the pointer, a triple-click the line.
                        const std::size_t c = static_cast<std::size_t>(n->caret);
                        std::size_t a = c, b = c;
                        if (ev.clicks >= 3) { a = line_start_of(n->value, c); b = line_end_of(n->value, c); }
                        else word_bounds(n->value, c, a, b);
                        n->anchor = static_cast<int>(a);
                        n->caret = static_cast<int>(b);
                    }
                } else {
                    set_focus_to(0);
                }
                NodeId t = im.listener_target(h, kMouseDown);
                if (t) emit(t, "mousedown", with_mods(Json{{"x", ev.x}, {"y", ev.y}, {"button", ev.button}}, ev.mods));
                break;
            }
            case EventType::MouseUp: {
                NodeId h = hit_test(ev.x, ev.y);
                NodeId ut = im.listener_target(h, kMouseUp);
                if (ut) emit(ut, "mouseup", Json{{"x", ev.x}, {"y", ev.y}, {"button", ev.button}});
                if (im.pressed && h == im.pressed && !im.dragging) {
                    NodeId t = im.listener_target(h, kClick);
                    Node* n = im.get(t);
                    if (t && !(n && n->disabled)) emit(t, "click", with_mods(Json{{"x", ev.x}, {"y", ev.y}, {"button", ev.button}}, ev.mods));
                } else if (im.pressed && im.dragging) {
                    NodeId dt = im.listener_target(im.pressed, kDrag);
                    if (dt) emit(dt, "dragend", Json{{"x", ev.x}, {"y", ev.y}});
                }
                im.pressed = 0;
                im.dragging = false;
                break;
            }
            case EventType::MouseWheel: {
                NodeId h = hit_test(im.last_x, im.last_y);
                // Scroll the nearest scrollable ancestor.
                NodeId s = h;
                while (s) {
                    Node* n = im.get(s);
                    if (!n) break;
                    if ((n->scroll || (n->type == "input" && n->multiline)) && n->content_height > n->rect.h) {
                        float max_scroll = n->content_height - n->rect.h;
                        n->scroll_y = std::clamp(n->scroll_y - ev.dy * 24.0f, 0.0f, max_scroll);
                        break;
                    }
                    s = n->parent;
                }
                NodeId t = im.listener_target(h, kWheel);
                if (t) emit(t, "wheel", Json{{"dx", ev.dx}, {"dy", ev.dy}});
                break;
            }
            case EventType::KeyDown: {
                Node* n = im.get(im.focused);
                bool consumed = false;
                if (n && n->type == "input" && !n->disabled) {
                    std::string& v = n->value;
                    auto caret = static_cast<std::size_t>(std::clamp(n->caret, 0, static_cast<int>(v.size())));
                    const bool shift = (ev.mods & platform::kModShift) != 0;
                    const bool cmd = (ev.mods & (platform::kModMeta | platform::kModCtrl)) != 0;
                    std::size_t sel_a = caret, sel_b = caret;
                    const bool has_sel = Impl::selection_of(*n, sel_a, sel_b);
                    bool edited = false;   // the value changed: relayout and an input event
                    auto erase_selection = [&]() { v.erase(sel_a, sel_b - sel_a); caret = sel_a; n->anchor = -1; edited = true; };
                    const bool moving = ev.key_name == "Left" || ev.key_name == "Right" || ev.key_name == "Home" || ev.key_name == "End" || (n->multiline && (ev.key_name == "Up" || ev.key_name == "Down"));
                    if (moving) {
                        // Shift extends the selection from where the caret was; without it a selection collapses.
                        if (shift && n->anchor < 0) n->anchor = static_cast<int>(caret);
                        if (ev.key_name == "Left") caret = !shift && has_sel ? sel_a : utf8_prev(v, caret);
                        else if (ev.key_name == "Right") caret = !shift && has_sel ? sel_b : utf8_next(v, caret);
                        else if (!n->multiline && ev.key_name == "Home") caret = 0;
                        else if (!n->multiline && ev.key_name == "End") caret = v.size();
                        else {
                            // Along the row (a line, or the wrapped part of one) for Home and End; the nearest
                            // position over or under the caret on the row above or below (the ends when there is none).
                            std::vector<Impl::Row> rows;
                            im.rows_of(*n, v, im.inner_width(*n), rows);
                            const std::size_t ri = Impl::row_of(rows, caret);
                            const float x = im.font.measure(v.substr(rows[ri].start, caret - rows[ri].start), n->font_size * im.scale);
                            if (ev.key_name == "Home") caret = rows[ri].start;
                            else if (ev.key_name == "End") caret = Impl::row_last(v, rows[ri]);
                            else if (ev.key_name == "Up") caret = ri == 0 ? 0 : im.offset_in_row(*n, v, rows[ri - 1], x);
                            else caret = ri + 1 >= rows.size() ? v.size() : im.offset_in_row(*n, v, rows[ri + 1], x);
                        }
                        if (!shift) n->anchor = -1;
                        consumed = true;
                    } else if (ev.key_name == "Backspace") {
                        if (has_sel) erase_selection();
                        else if (caret > 0) { std::size_t p = utf8_prev(v, caret); v.erase(p, caret - p); caret = p; edited = true; }
                        consumed = true;
                    } else if (ev.key_name == "Delete") {
                        if (has_sel) erase_selection();
                        else if (caret < v.size()) { v.erase(caret, utf8_next(v, caret) - caret); edited = true; }
                        consumed = true;
                    } else if (cmd && ev.key_name == "A") {
                        n->anchor = 0;
                        caret = v.size();
                        consumed = true;
                    } else if (cmd && (ev.key_name == "C" || ev.key_name == "X")) {
                        if (has_sel) {
                            im.clipboard_set(v.substr(sel_a, sel_b - sel_a));
                            if (ev.key_name == "X") erase_selection();
                        }
                        consumed = true;
                    } else if (cmd && ev.key_name == "V") {
                        std::string text = im.clipboard_get();
                        if (!n->multiline) std::replace(text.begin(), text.end(), '\n', ' ');
                        if (has_sel) erase_selection();
                        if (!text.empty()) { v.insert(caret, text); caret += text.size(); edited = true; }
                        consumed = true;
                    } else if (ev.key_name == "Return" || ev.key_name == "Keypad Enter") {
                        if (!n->multiline || cmd) {
                            NodeId ct = im.listener_target(im.focused, kChange);
                            if (ct) emit(ct, "change", Json{{"value", v}});
                        } else {
                            if (has_sel) erase_selection();
                            v.insert(caret, "\n");
                            ++caret;
                            edited = true;
                        }
                        consumed = true;
                    } else if (ev.key_name == "Escape") { set_focus_to(0); consumed = true; }
                    n->caret = static_cast<int>(caret);
                    if (edited) {
                        if (YGNodeHasMeasureFunc(n->yoga)) YGNodeMarkDirty(n->yoga);
                        NodeId it = im.listener_target(im.focused, kInput);
                        if (it) emit(it, "input", Json{{"value", v}});
                    }
                }
                if (!consumed && ev.key_name == "Tab") {
                    // Tab walks the focus through the inputs and the elements that listen for clicks,
                    // in tree order, wrapping at the ends; Shift+Tab walks back.
                    bool shift = false;
                    for (const Json& m : platform::mods_to_json(ev.mods)) if (m == "shift") shift = true;
                    std::vector<NodeId> order;
                    std::function<void(NodeId)> collect = [&](NodeId id) {
                        const Node* node = im.get(id);
                        if (!node || !Impl::shown(*node)) return;
                        if (!node->disabled && node->rect.w > 0 && node->rect.h > 0 && (node->type == "input" || (node->listeners & kClick))) order.push_back(id);
                        for (NodeId c : node->children) collect(c);
                    };
                    collect(im.root_id);
                    if (!order.empty()) {
                        std::size_t at = order.size();
                        for (std::size_t i = 0; i < order.size(); ++i) if (order[i] == im.focused) at = i;
                        NodeId next = at == order.size() ? (shift ? order.back() : order.front()) : order[(at + (shift ? order.size() - 1 : 1)) % order.size()];
                        set_focus_to(next);
                    }
                    consumed = true;
                } else if (!consumed && n && n->type != "input" && !n->disabled && (n->listeners & kClick) && (ev.key_name == "Return" || ev.key_name == "Keypad Enter" || ev.key_name == "Space")) {
                    // A focused button pressed from the keyboard: a click at its center.
                    emit(im.focused, "click", with_mods(Json{{"x", n->rect.x + n->rect.w * 0.5f}, {"y", n->rect.y + n->rect.h * 0.5f}, {"keyboard", true}}, ev.mods));
                    consumed = true;
                }
                NodeId t = im.listener_target(im.focused ? im.focused : im.hovered, kKeyDown);
                if (t) emit(t, "keydown", with_mods(Json{{"key", ev.key_name}, {"repeat", ev.repeat}, {"consumed", consumed}}, ev.mods));
                else if (!consumed) {
                    // Unfocused keys go to the root listener if any.
                    NodeId rt = im.listener_target(im.root_id, kKeyDown);
                    if (rt) emit(rt, "keydown", with_mods(Json{{"key", ev.key_name}, {"repeat", ev.repeat}, {"consumed", false}}, ev.mods));
                }
                break;
            }
            case EventType::Text: {
                Node* n = im.get(im.focused);
                if (n && n->type == "input" && !n->disabled) {
                    auto caret = static_cast<std::size_t>(std::clamp(n->caret, 0, static_cast<int>(n->value.size())));
                    if (std::size_t a = 0, b = 0; Impl::selection_of(*n, a, b)) { n->value.erase(a, b - a); caret = a; }
                    n->anchor = -1;
                    n->value.insert(caret, ev.text);
                    n->caret = static_cast<int>(caret + ev.text.size());
                    if (YGNodeHasMeasureFunc(n->yoga)) YGNodeMarkDirty(n->yoga);
                    NodeId it = im.listener_target(im.focused, kInput);
                    if (it) emit(it, "input", Json{{"value", n->value}});
                }
                break;
            }
            default:
                break;
        }
    }
    const Node* f = im.get(im.focused);
    text_input_wanted = f && f->type == "input";
    return out;
}

Json Document::describe(NodeId id) const {
    const Node* n = impl_->get(id);
    Json j;
    if (!n) { j["id"] = id; j["exists"] = false; return j; }
    j["id"] = id;
    j["type"] = n->type;
    if (!n->name.empty()) j["name"] = n->name;
    j["parent"] = n->parent;
    j["children"] = n->children;
    j["rect"] = Json{{"x", n->rect.x}, {"y", n->rect.y}, {"w", n->rect.w}, {"h", n->rect.h}};
    if (n->type == "text") j["text"] = n->text;
    if (n->type == "input") {
        j["value"] = n->value;
        j["placeholder"] = n->placeholder;
        j["caret"] = n->caret;
        if (!n->syntax.empty()) j["syntax"] = std::string(language_of(n->syntax));
        if (n->multiline) {
            j["multiline"] = true;
            if (n->text_wrap) j["wrap"] = true;
            std::vector<Impl::Row> rows;
            impl_->rows_of(*n, n->value, impl_->inner_width(*n), rows);
            j["rows"] = rows.size();
            j["scrollTop"] = n->scroll_y;
            j["contentHeight"] = n->content_height;
        }
        if (std::size_t a = 0, b = 0; Impl::selection_of(*n, a, b)) j["selection"] = Json::array({a, b});
    }
    j["visible"] = Impl::shown(*n);
    if (n->anchor_entity) {
        j["anchor"] = n->anchor_entity;
        j["anchorOffset"] = Json::array({n->anchor_offset[0], n->anchor_offset[1]});
        j["anchorAlign"] = Json::array({n->anchor_align[0], n->anchor_align[1]});
    }
    j["focused"] = impl_->focused == id;
    if (n->scroll) { j["scrollTop"] = n->scroll_y; j["contentHeight"] = n->content_height; }
    j["background"] = color_hex(n->background);
    j["color"] = color_hex(n->color);
    j["opacity"] = n->opacity;
    if (n->keyframes) {
        const Node::Keyframes& a = *n->keyframes;
        Json props = Json::array();
        for (const auto& [k, t] : a.tracks) props.push_back(k);
        j["animation"] = Json{{"time", a.t}, {"duration", a.duration * 1000.0f}, {"iterations", a.iterations}, {"finished", a.finished}, {"props", props}};
    }
    if (!n->transition.empty()) {
        Json tr = Json::object();
        for (const auto& [k, secs] : n->transition) tr[k] = secs * 1000.0f;
        j["transition"] = tr;
        if (!n->anims.empty()) {
            Json running = Json::array();
            for (const auto& [k, a] : n->anims) running.push_back(k);
            j["animating"] = running;
        }
    }
    if (!n->image.empty()) {
        j["image"] = n->image;
        j["fit"] = n->fit;
        if (n->slice[0] > 0 || n->slice[1] > 0 || n->slice[2] > 0 || n->slice[3] > 0) j["slice"] = Json::array({n->slice[0], n->slice[1], n->slice[2], n->slice[3]});
        if (n->filter == "nearest") j["filter"] = "nearest";
    }
    return j;
}

Rect Document::caret_rect() const {
    const Node* n = impl_->get(impl_->focused);
    return n && n->type == "input" ? impl_->caret_rect : Rect{};
}

Rect Document::rect_of(NodeId id) const {
    const Node* n = impl_->get(id);
    return n ? n->rect : Rect{};
}

std::string Document::snapshot(const SnapshotOptions& options) const {
    std::ostringstream out;
    const Node* start = impl_->get(options.root ? options.root : impl_->root_id);
    if (!start) return "no such node\n";
    out << "ui: " << impl_->nodes.size() << " nodes, " << std::lround(impl_->width) << "x" << std::lround(impl_->height) << " points" << (impl_->focused ? " focused #" + std::to_string(impl_->focused) : "") << "\n";
    int shown = 0;
    impl_->snapshot_node(*start, 0, options, shown, out);
    return out.str();
}

Json Document::query(const Json& params) const {
    std::string type = params.value("type", "");
    std::string text = params.value("text", "");
    std::string name = params.value("name", "");
    Json results = Json::array();
    for (const auto& [id, n] : impl_->nodes) {
        if (!type.empty() && n.type != type) continue;
        if (!name.empty() && n.name != name) continue;
        if (!text.empty()) {
            const std::string& hay = n.type == "input" ? n.value : n.text;
            if (hay.find(text) == std::string::npos) continue;
        }
        Json r;
        r["id"] = id;
        r["type"] = n.type;
        if (!n.name.empty()) r["name"] = n.name;
        if (n.type == "text") r["text"] = n.text;
        if (n.type == "input") r["value"] = n.value;
        r["rect"] = Json{{"x", n.rect.x}, {"y", n.rect.y}, {"w", n.rect.w}, {"h", n.rect.h}};
        results.push_back(r);
    }
    return results;
}

Json Document::stats() const {
    Json j;
    j["nodes"] = impl_->nodes.size();
    j["focused"] = impl_->focused;
    j["hovered"] = impl_->hovered;
    j["paints"] = impl_->paints;
    return j;
}

}  // namespace pocket::ui

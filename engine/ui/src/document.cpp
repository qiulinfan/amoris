#include <pocket/ui/document.hpp>

#include <pocket/core/log.hpp>

#include <yoga/Yoga.h>

#include <algorithm>
#include <cmath>
#include <functional>
#include <map>
#include <sstream>

namespace pocket::ui {

namespace {

enum Listener : std::uint32_t {
    kClick = 1u << 0, kInput = 1u << 1, kChange = 1u << 2, kKeyDown = 1u << 3, kWheel = 1u << 4,
    kHover = 1u << 5, kFocus = 1u << 6, kDrag = 1u << 7, kMouseDown = 1u << 8, kMouseUp = 1u << 9,
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
    bool disabled = false;
    std::string text;
    std::string value;       // input
    std::string placeholder;
    int caret = 0;
    bool multiline = false;  // input: Return is a new line (with meta or ctrl it commits), Up and Down move by lines
    int first_line = 0;      // multiline input: the first line shown, kept so the caret's line stays in view
    std::uint32_t listeners = 0;
    // Layout results (absolute, points)
    Rect rect;
    float scroll_y = 0;
    float content_height = 0;
    // Cached wrapped lines for text nodes
    std::vector<std::string> lines;
    float measured_width = -1;
};

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

}  // namespace

struct Document::Impl {
    Font& font;
    std::function<Document::ImageSource(const std::string&)> image_source;
    std::map<NodeId, Node> nodes;
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
        if (n->type == "input" && n->multiline) split_lines(text, lines);
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

    void apply_style(Node& n, const Json& props) {
        YGNodeRef y = n.yoga;
        for (auto& [k, v] : props.items()) {
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
            else if (k == "display") { std::string s = v.get<std::string>(); n.visible = s != "none"; YGNodeStyleSetDisplay(y, n.visible ? YGDisplayFlex : YGDisplayNone); }
            else if (k == "background" || k == "backgroundColor" || k == "bg") n.background = parse_color(v, n.background);
            else if (k == "image") n.image = v.is_string() ? v.get<std::string>() : "";
            else if (k == "fit") n.fit = v.is_string() ? v.get<std::string>() : "contain";
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
            else if (k == "value") { n.value = v.is_string() ? v.get<std::string>() : v.dump(); n.caret = static_cast<int>(n.value.size()); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
            else if (k == "placeholder") n.placeholder = v.get<std::string>();
            else if (k == "multiline") { n.multiline = v.is_boolean() && v.get<bool>(); if (YGNodeHasMeasureFunc(y)) YGNodeMarkDirty(y); }
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
        if (!n.visible) return;
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
                    p.image_sliced(r, src.view, u0, v0, u1, v1, pw, ph, n.slice[0], n.slice[1], n.slice[2], n.slice[3], Color{1, 1, 1, op});
                } else if (n.fit == "contain" && pw > 0 && ph > 0) {
                    const float s = std::min(r.w / pw, r.h / ph);
                    box = {r.x + (r.w - pw * s) * 0.5f, r.y + (r.h - ph * s) * 0.5f, pw * s, ph * s};
                } else if (n.fit == "cover" && pw > 0 && ph > 0) {
                    const float s = std::max(r.w / pw, r.h / ph);
                    const float vw = r.w / (s * src.width), vh = r.h / (s * src.height);   // the part that shows, in uv
                    const float cu = (u0 + u1) * 0.5f, cv = (v0 + v1) * 0.5f;
                    u0 = cu - vw * 0.5f; u1 = cu + vw * 0.5f; v0 = cv - vh * 0.5f; v1 = cv + vh * 0.5f;
                }
                if (n.slice[0] <= 0 && n.slice[1] <= 0 && n.slice[2] <= 0 && n.slice[3] <= 0) p.image(box, src.view, u0, v0, u1, v1, Color{1, 1, 1, op}, n.radius);
            }
        }
        if (n.border_width > 0 && n.border_color.a > 0) p.border(r, n.border_color.with_alpha(op), n.border_width, n.radius);
        if (n.type == "text" || n.type == "input") {
            Rect inner{r.x + YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) + n.border_width, r.y + YGNodeLayoutGetPadding(n.yoga, YGEdgeTop) + n.border_width, r.w - YGNodeLayoutGetPadding(n.yoga, YGEdgeLeft) - YGNodeLayoutGetPadding(n.yoga, YGEdgeRight) - 2 * n.border_width, r.h - YGNodeLayoutGetPadding(n.yoga, YGEdgeTop) - YGNodeLayoutGetPadding(n.yoga, YGEdgeBottom) - 2 * n.border_width};
            p.push_clip(inner);
            float lh = p.line_height(n.font_size);
            if (n.type == "input" && n.multiline) {
                // Lines from the top, the first shown chosen so the caret's line is in view; the caret on its line.
                bool placeholder = n.value.empty();
                Color c = placeholder ? n.color.with_alpha(0.45f * op) : n.color.with_alpha(op);
                std::vector<std::string> lines;
                split_lines(placeholder ? n.placeholder : n.value, lines);
                const std::size_t caret = static_cast<std::size_t>(std::clamp(n.caret, 0, static_cast<int>(n.value.size())));
                int caret_line = 0;
                std::size_t line_start = 0;
                for (std::size_t i = 0; i < caret && i < n.value.size(); ++i) if (n.value[i] == '\n') { ++caret_line; line_start = i + 1; }
                const int visible = std::max(1, static_cast<int>(std::floor((inner.h - 4) / std::max(lh, 1.0f))));
                if (caret_line < n.first_line) n.first_line = caret_line;
                if (caret_line >= n.first_line + visible) n.first_line = caret_line - visible + 1;
                n.first_line = std::clamp(n.first_line, 0, std::max(0, static_cast<int>(lines.size()) - 1));
                float ty = inner.y + 2;
                for (std::size_t i = static_cast<std::size_t>(n.first_line); i < lines.size() && ty < inner.y + inner.h; ++i) {
                    p.text(inner.x + 1, ty, lines[i], n.font_size, c);
                    ty += lh;
                }
                if (focused == n.id && !placeholder) {
                    const float cx = inner.x + 1 + p.measure(n.value.substr(line_start, caret - line_start), n.font_size);
                    const float cy = inner.y + 2 + static_cast<float>(caret_line - n.first_line) * lh;
                    p.rect({cx, cy + 1, 1, lh - 2}, n.color.with_alpha(op));
                } else if (focused == n.id) {
                    p.rect({inner.x + 1, inner.y + 3, 1, lh - 2}, n.color.with_alpha(op));
                }
            } else if (n.type == "input") {
                bool placeholder = n.value.empty();
                const std::string& shown = placeholder ? n.placeholder : n.value;
                Color c = placeholder ? n.color.with_alpha(0.45f * op) : n.color.with_alpha(op);
                float ty = inner.y + (inner.h - lh) * 0.5f;
                p.text(inner.x + 1, ty, shown, n.font_size, c);
                if (focused == n.id) {
                    float cx = inner.x + 1 + p.measure(n.value.substr(0, static_cast<std::size_t>(std::clamp(n.caret, 0, static_cast<int>(n.value.size())))), n.font_size);
                    p.rect({cx, ty + 1, 1, lh - 2}, n.color.with_alpha(op));
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
        if (!n.visible) return 0;
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
        if (!n.visible) out << " hidden";
        if (n.scroll) out << " scroll=" << std::lround(n.scroll_y) << "/" << std::lround(std::max(0.0f, n.content_height - n.rect.h));
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
    YGNodeCalculateLayout(root.yoga, width, height, YGDirectionLTR);
    im.compute_rects(root, 0, 0);
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
    std::vector<Json> out;
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
                    set_focus_to(h);
                    // Place the caret near the click.
                    float px = n->font_size * im.scale;
                    float local = (ev.x - n->rect.x - YGNodeLayoutGetPadding(n->yoga, YGEdgeLeft) - n->border_width - 1) * im.scale;
                    std::size_t best = n->value.size();
                    for (std::size_t i = 0; i <= n->value.size(); i = utf8_next(n->value, i)) {
                        float w = im.font.measure(n->value.substr(0, i), px);
                        if (w >= local) { best = i; break; }
                        if (i == n->value.size()) break;
                    }
                    n->caret = static_cast<int>(best);
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
                    if (n->scroll && n->content_height > n->rect.h) {
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
                    if (ev.key_name == "Backspace") {
                        if (caret > 0) { std::size_t p = utf8_prev(v, caret); v.erase(p, caret - p); caret = p; }
                        consumed = true;
                    } else if (ev.key_name == "Delete") {
                        if (caret < v.size()) v.erase(caret, utf8_next(v, caret) - caret);
                        consumed = true;
                    } else if (ev.key_name == "Left") { caret = utf8_prev(v, caret); consumed = true; }
                    else if (ev.key_name == "Right") { caret = utf8_next(v, caret); consumed = true; }
                    else if (ev.key_name == "Home") { caret = n->multiline ? line_start_of(v, caret) : 0; consumed = true; }
                    else if (ev.key_name == "End") { caret = n->multiline ? line_end_of(v, caret) : v.size(); consumed = true; }
                    else if (n->multiline && (ev.key_name == "Up" || ev.key_name == "Down")) {
                        // The same column on the line above or below (or the ends when there is none).
                        const std::size_t ls = line_start_of(v, caret), col = caret - ls;
                        if (ev.key_name == "Up") {
                            if (ls == 0) caret = 0;
                            else { const std::size_t ps = line_start_of(v, ls - 1); caret = std::min(ps + col, ls - 1); }
                        } else {
                            const std::size_t le = line_end_of(v, caret);
                            if (le >= v.size()) caret = v.size();
                            else { const std::size_t ns = le + 1; caret = std::min(ns + col, line_end_of(v, ns)); }
                        }
                        consumed = true;
                    } else if (ev.key_name == "Return" || ev.key_name == "Keypad Enter") {
                        bool commit = !n->multiline;
                        for (const Json& m : platform::mods_to_json(ev.mods)) if (m == "meta" || m == "ctrl") commit = true;
                        if (commit) {
                            NodeId ct = im.listener_target(im.focused, kChange);
                            if (ct) emit(ct, "change", Json{{"value", v}});
                        } else {
                            v.insert(caret, "\n");
                            ++caret;
                            if (YGNodeHasMeasureFunc(n->yoga)) YGNodeMarkDirty(n->yoga);
                            NodeId it = im.listener_target(im.focused, kInput);
                            if (it) emit(it, "input", Json{{"value", v}});
                        }
                        consumed = true;
                    } else if (ev.key_name == "Escape") { set_focus_to(0); consumed = true; }
                    n->caret = static_cast<int>(caret);
                    if (consumed && (ev.key_name == "Backspace" || ev.key_name == "Delete")) {
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
                        if (!node || !node->visible) return;
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
    if (n->type == "input") { j["value"] = n->value; j["placeholder"] = n->placeholder; j["caret"] = n->caret; if (n->multiline) j["multiline"] = true; }
    j["visible"] = n->visible;
    j["focused"] = impl_->focused == id;
    if (n->scroll) { j["scrollTop"] = n->scroll_y; j["contentHeight"] = n->content_height; }
    j["background"] = color_hex(n->background);
    j["color"] = color_hex(n->color);
    if (!n->image.empty()) {
        j["image"] = n->image;
        j["fit"] = n->fit;
        if (n->slice[0] > 0 || n->slice[1] > 0 || n->slice[2] > 0 || n->slice[3] > 0) j["slice"] = Json::array({n->slice[0], n->slice[1], n->slice[2], n->slice[3]});
    }
    return j;
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

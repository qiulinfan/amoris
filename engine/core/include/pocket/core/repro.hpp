// Reproducible math: the same float from every build of the engine.
//
// The platform's libm rounds differently in the last bit (macOS's sinf is not the musl sinf a web
// build has), so the peers of a lockstep game running different builds would drift apart an ulp at
// a time (docs/design/networking.md, Determinism). These functions use only operations IEEE 754
// defines exactly (+ - * / and sqrt, in double, rounded to float once at the end) and so give the
// same bits on every platform; they are about as accurate as a float libm (within an ulp or so).
// Simulation code (physics, water, characters, rigs, timelines, navigation, particles) uses them;
// drawing, which no peer compares, may use <cmath>.
#pragma once

namespace pocket::repro {

[[nodiscard]] float sin(float x);
[[nodiscard]] float cos(float x);
void sincos(float x, float& s, float& c);
[[nodiscard]] float tan(float x);
[[nodiscard]] float atan(float x);
[[nodiscard]] float atan2(float y, float x);
[[nodiscard]] float asin(float x);
[[nodiscard]] float acos(float x);
[[nodiscard]] float exp(float x);
[[nodiscard]] float log(float x);
[[nodiscard]] float pow(float x, float y);
[[nodiscard]] float cbrt(float x);
[[nodiscard]] float hypot(float x, float y);

}  // namespace pocket::repro

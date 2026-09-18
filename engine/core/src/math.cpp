#include <pocket/core/math.hpp>

namespace pocket {

Mat4 Mat4::inverse_affine() const {
    // Invert the 3x3 linear part then the translation.
    float a00 = at(0, 0), a01 = at(1, 0), a02 = at(2, 0);
    float a10 = at(0, 1), a11 = at(1, 1), a12 = at(2, 1);
    float a20 = at(0, 2), a21 = at(1, 2), a22 = at(2, 2);
    float det = a00 * (a11 * a22 - a12 * a21) - a01 * (a10 * a22 - a12 * a20) + a02 * (a10 * a21 - a11 * a20);
    Mat4 r;
    if (det == 0.0f) return r;
    float inv = 1.0f / det;
    float b00 = (a11 * a22 - a12 * a21) * inv;
    float b01 = (a02 * a21 - a01 * a22) * inv;
    float b02 = (a01 * a12 - a02 * a11) * inv;
    float b10 = (a12 * a20 - a10 * a22) * inv;
    float b11 = (a00 * a22 - a02 * a20) * inv;
    float b12 = (a02 * a10 - a00 * a12) * inv;
    float b20 = (a10 * a21 - a11 * a20) * inv;
    float b21 = (a01 * a20 - a00 * a21) * inv;
    float b22 = (a00 * a11 - a01 * a10) * inv;
    r.at(0, 0) = b00; r.at(1, 0) = b01; r.at(2, 0) = b02;
    r.at(0, 1) = b10; r.at(1, 1) = b11; r.at(2, 1) = b12;
    r.at(0, 2) = b20; r.at(1, 2) = b21; r.at(2, 2) = b22;
    float tx = at(3, 0), ty = at(3, 1), tz = at(3, 2);
    r.at(3, 0) = -(b00 * tx + b01 * ty + b02 * tz);
    r.at(3, 1) = -(b10 * tx + b11 * ty + b12 * tz);
    r.at(3, 2) = -(b20 * tx + b21 * ty + b22 * tz);
    return r;
}

}  // namespace pocket

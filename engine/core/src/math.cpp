#include <pocket/core/math.hpp>

namespace pocket {

Mat4 Mat4::inverse() const {
    // Cofactor expansion over the 16 entries; the layout does not matter as long as it is consistent.
    const float* a = m;
    float inv[16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15] + a[9] * a[7] * a[14] + a[13] * a[6] * a[11] - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15] - a[8] * a[7] * a[14] - a[12] * a[6] * a[11] + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15] + a[8] * a[7] * a[13] + a[12] * a[5] * a[11] - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14] - a[8] * a[6] * a[13] - a[12] * a[5] * a[10] + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15] - a[9] * a[3] * a[14] - a[13] * a[2] * a[11] + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15] + a[8] * a[3] * a[14] + a[12] * a[2] * a[11] - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15] - a[8] * a[3] * a[13] - a[12] * a[1] * a[11] + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14] + a[8] * a[2] * a[13] + a[12] * a[1] * a[10] - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15] + a[5] * a[3] * a[14] + a[13] * a[2] * a[7] - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15] - a[4] * a[3] * a[14] - a[12] * a[2] * a[7] + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15] + a[4] * a[3] * a[13] + a[12] * a[1] * a[7] - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14] - a[4] * a[2] * a[13] - a[12] * a[1] * a[6] + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11] - a[5] * a[3] * a[10] - a[9] * a[2] * a[7] + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11] + a[4] * a[3] * a[10] + a[8] * a[2] * a[7] - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11] - a[4] * a[3] * a[9] - a[8] * a[1] * a[7] + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10] + a[4] * a[2] * a[9] + a[8] * a[1] * a[6] - a[8] * a[2] * a[5];
    float det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    Mat4 r;
    if (det == 0.0f) return r;
    float s = 1.0f / det;
    for (int i = 0; i < 16; ++i) r.m[i] = inv[i] * s;
    return r;
}

void decompose(const Mat4& m, Vec3& t, Quat& r, Vec3& s) {
    t = {m.at(3, 0), m.at(3, 1), m.at(3, 2)};
    Vec3 c0{m.at(0, 0), m.at(0, 1), m.at(0, 2)}, c1{m.at(1, 0), m.at(1, 1), m.at(1, 2)}, c2{m.at(2, 0), m.at(2, 1), m.at(2, 2)};
    s = {length(c0), length(c1), length(c2)};
    if (s.x > 0) c0 = c0 * (1.0f / s.x);
    if (s.y > 0) c1 = c1 * (1.0f / s.y);
    if (s.z > 0) c2 = c2 * (1.0f / s.z);
    const float tr = c0.x + c1.y + c2.z;
    if (tr > 0) {
        const float k = std::sqrt(tr + 1.0f) * 2.0f;
        r = {(c1.z - c2.y) / k, (c2.x - c0.z) / k, (c0.y - c1.x) / k, 0.25f * k};
    } else if (c0.x > c1.y && c0.x > c2.z) {
        const float k = std::sqrt(1.0f + c0.x - c1.y - c2.z) * 2.0f;
        r = {0.25f * k, (c1.x + c0.y) / k, (c2.x + c0.z) / k, (c1.z - c2.y) / k};
    } else if (c1.y > c2.z) {
        const float k = std::sqrt(1.0f + c1.y - c0.x - c2.z) * 2.0f;
        r = {(c1.x + c0.y) / k, 0.25f * k, (c2.y + c1.z) / k, (c2.x - c0.z) / k};
    } else {
        const float k = std::sqrt(1.0f + c2.z - c0.x - c1.y) * 2.0f;
        r = {(c2.x + c0.z) / k, (c2.y + c1.z) / k, 0.25f * k, (c0.y - c1.x) / k};
    }
}

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

// Small linear algebra: Vec2/3/4, Quat, Mat4 (column-major, right-handed, Y up, -Z forward).
// Doubles are not used in the hot path; simulation state is float32 so hashes are portable.
#pragma once

#include <cmath>
#include <cstdint>
#include <numbers>

#include <pocket/core/repro.hpp>

namespace pocket {

struct Vec2 {
    float x = 0, y = 0;
    constexpr Vec2 operator+(Vec2 o) const { return {x + o.x, y + o.y}; }
    constexpr Vec2 operator-(Vec2 o) const { return {x - o.x, y - o.y}; }
    constexpr Vec2 operator*(float s) const { return {x * s, y * s}; }
    constexpr bool operator==(const Vec2&) const = default;
};

struct Vec3 {
    float x = 0, y = 0, z = 0;
    constexpr Vec3 operator+(Vec3 o) const { return {x + o.x, y + o.y, z + o.z}; }
    constexpr Vec3 operator-(Vec3 o) const { return {x - o.x, y - o.y, z - o.z}; }
    constexpr Vec3 operator*(float s) const { return {x * s, y * s, z * s}; }
    constexpr Vec3 operator-() const { return {-x, -y, -z}; }
    constexpr Vec3& operator+=(Vec3 o) { x += o.x; y += o.y; z += o.z; return *this; }
    constexpr Vec3& operator-=(Vec3 o) { x -= o.x; y -= o.y; z -= o.z; return *this; }
    constexpr Vec3& operator*=(float s) { x *= s; y *= s; z *= s; return *this; }
    constexpr bool operator==(const Vec3&) const = default;
};

struct Vec4 {
    float x = 0, y = 0, z = 0, w = 0;
    constexpr bool operator==(const Vec4&) const = default;
};

constexpr float dot(Vec3 a, Vec3 b) { return a.x * b.x + a.y * b.y + a.z * b.z; }
constexpr Vec3 cross(Vec3 a, Vec3 b) { return {a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x}; }
inline float length(Vec3 v) { return std::sqrt(dot(v, v)); }
inline Vec3 normalize(Vec3 v) {
    float l = length(v);
    return l > 0 ? v * (1.0f / l) : Vec3{};
}
constexpr Vec3 lerp(Vec3 a, Vec3 b, float t) { return a + (b - a) * t; }

struct Quat {
    float x = 0, y = 0, z = 0, w = 1;
    static Quat from_axis_angle(Vec3 axis, float radians) {
        Vec3 n = normalize(axis);
        float s, c;
        repro::sincos(radians * 0.5f, s, c);
        return {n.x * s, n.y * s, n.z * s, c};
    }
    static Quat from_euler(Vec3 radians_xyz) {  // yaw(Y) * pitch(X) * roll(Z)
        Quat qx = from_axis_angle({1, 0, 0}, radians_xyz.x);
        Quat qy = from_axis_angle({0, 1, 0}, radians_xyz.y);
        Quat qz = from_axis_angle({0, 0, 1}, radians_xyz.z);
        return qy * qx * qz;
    }
    constexpr Quat operator*(Quat o) const {
        return {w * o.x + x * o.w + y * o.z - z * o.y, w * o.y - x * o.z + y * o.w + z * o.x,
                w * o.z + x * o.y - y * o.x + z * o.w, w * o.w - x * o.x - y * o.y - z * o.z};
    }
    Vec3 rotate(Vec3 v) const {
        Vec3 u{x, y, z};
        Vec3 t = cross(u, v) * 2.0f;
        return v + t * w + cross(u, t);
    }
    constexpr bool operator==(const Quat&) const = default;
};

inline Quat normalize(Quat q) {
    float l = std::sqrt(q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w);
    return l > 0 ? Quat{q.x / l, q.y / l, q.z / l, q.w / l} : Quat{};
}

struct Mat4 {
    float m[16] = {1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1};  // column-major

    constexpr float& at(int col, int row) { return m[col * 4 + row]; }
    constexpr float at(int col, int row) const { return m[col * 4 + row]; }

    static constexpr Mat4 identity() { return {}; }

    static Mat4 translation(Vec3 t) {
        Mat4 r;
        r.at(3, 0) = t.x;
        r.at(3, 1) = t.y;
        r.at(3, 2) = t.z;
        return r;
    }
    static Mat4 scale(Vec3 s) {
        Mat4 r;
        r.at(0, 0) = s.x;
        r.at(1, 1) = s.y;
        r.at(2, 2) = s.z;
        return r;
    }
    static Mat4 rotation(Quat q) {
        Mat4 r;
        float xx = q.x * q.x, yy = q.y * q.y, zz = q.z * q.z;
        float xy = q.x * q.y, xz = q.x * q.z, yz = q.y * q.z;
        float wx = q.w * q.x, wy = q.w * q.y, wz = q.w * q.z;
        r.at(0, 0) = 1 - 2 * (yy + zz); r.at(0, 1) = 2 * (xy + wz);     r.at(0, 2) = 2 * (xz - wy);
        r.at(1, 0) = 2 * (xy - wz);     r.at(1, 1) = 1 - 2 * (xx + zz); r.at(1, 2) = 2 * (yz + wx);
        r.at(2, 0) = 2 * (xz + wy);     r.at(2, 1) = 2 * (yz - wx);     r.at(2, 2) = 1 - 2 * (xx + yy);
        return r;
    }
    static Mat4 trs(Vec3 t, Quat q, Vec3 s) { return translation(t) * rotation(q) * scale(s); }

    // Right-handed, looking down -Z, depth range [0,1] (WebGPU/Metal convention).
    static Mat4 perspective(float fov_y_radians, float aspect, float near, float far) {
        float f = 1.0f / std::tan(fov_y_radians * 0.5f);
        Mat4 r;
        for (float& v : r.m) v = 0;
        r.at(0, 0) = f / aspect;
        r.at(1, 1) = f;
        r.at(2, 2) = far / (near - far);
        r.at(2, 3) = -1;
        r.at(3, 2) = (near * far) / (near - far);
        return r;
    }
    static Mat4 orthographic(float left, float right, float bottom, float top, float near, float far) {
        Mat4 r;
        r.at(0, 0) = 2 / (right - left);
        r.at(1, 1) = 2 / (top - bottom);
        r.at(2, 2) = 1 / (near - far);
        r.at(3, 0) = -(right + left) / (right - left);
        r.at(3, 1) = -(top + bottom) / (top - bottom);
        r.at(3, 2) = near / (near - far);
        return r;
    }
    static Mat4 look_at(Vec3 eye, Vec3 target, Vec3 up) {
        Vec3 f = normalize(target - eye);
        Vec3 s = normalize(cross(f, up));
        Vec3 u = cross(s, f);
        Mat4 r;
        r.at(0, 0) = s.x; r.at(1, 0) = s.y; r.at(2, 0) = s.z;
        r.at(0, 1) = u.x; r.at(1, 1) = u.y; r.at(2, 1) = u.z;
        r.at(0, 2) = -f.x; r.at(1, 2) = -f.y; r.at(2, 2) = -f.z;
        r.at(3, 0) = -dot(s, eye);
        r.at(3, 1) = -dot(u, eye);
        r.at(3, 2) = dot(f, eye);
        return r;
    }

    Mat4 operator*(const Mat4& o) const {
        Mat4 r;
        for (int c = 0; c < 4; ++c) {
            for (int row = 0; row < 4; ++row) {
                float sum = 0;
                for (int k = 0; k < 4; ++k) sum += at(k, row) * o.at(c, k);
                r.at(c, row) = sum;
            }
        }
        return r;
    }
    Vec4 operator*(Vec4 v) const {
        return {at(0, 0) * v.x + at(1, 0) * v.y + at(2, 0) * v.z + at(3, 0) * v.w,
                at(0, 1) * v.x + at(1, 1) * v.y + at(2, 1) * v.z + at(3, 1) * v.w,
                at(0, 2) * v.x + at(1, 2) * v.y + at(2, 2) * v.z + at(3, 2) * v.w,
                at(0, 3) * v.x + at(1, 3) * v.y + at(2, 3) * v.z + at(3, 3) * v.w};
    }
    Vec3 transform_point(Vec3 p) const {
        Vec4 r = *this * Vec4{p.x, p.y, p.z, 1};
        return {r.x, r.y, r.z};
    }
    Vec3 transform_dir(Vec3 d) const {
        Vec4 r = *this * Vec4{d.x, d.y, d.z, 0};
        return {r.x, r.y, r.z};
    }
    Mat4 inverse_affine() const;  // for TRS matrices without projection
    Mat4 inverse() const;         // any invertible matrix (projections included); identity when singular
};

// A matrix taken apart into a translation, a rotation and a scale (no shear).
void decompose(const Mat4& m, Vec3& t, Quat& r, Vec3& s);

constexpr float kPi = std::numbers::pi_v<float>;
constexpr float radians(float degrees) { return degrees * (kPi / 180.0f); }
constexpr float degrees(float rad) { return rad * (180.0f / kPi); }

}  // namespace pocket

//! CPU geometry checks for the analytic Smooth material footprint in forward.wgsl.
//! Known tangent planes are intersected by independently unprojected neighboring camera rays.
//! This checks the f32 formula against f64 geometry; actual GPU shader rounding is covered by
//! the renderer's water/LOD/morph image tests, not by this CPU reference implementation.
use amoris_render::CameraState;
use glam::{DMat4, DVec2, DVec3, DVec4, Mat4, Vec2, Vec3};

#[derive(Debug)]
struct Differential {
    dx: Vec3,
    dy: Vec3,
    footprint: f32,
    whole_field: bool,
}
// f32 candidate evaluated against independently unprojected f64 neighbor rays below.
fn candidate(
    view: Mat4,
    projection: Mat4,
    size: Vec2,
    pixel: Vec2,
    world: Vec3,
    normal: Vec3,
    extent: Vec2,
) -> Differential {
    let scale = extent.max_element();
    let limit = if scale > 0.0 && scale < 1e18 {
        (scale * (extent / scale).length()).max(1e-3)
    } else {
        1e18
    };
    let whole = || Differential {
        dx: Vec3::ZERO,
        dy: Vec3::ZERO,
        footprint: limit,
        whole_field: true,
    };
    let px = projection.x_axis.x;
    let py = projection.y_axis.y;
    let nn = normal.length_squared();
    if !(px.abs() > 1e-8
        && px.abs() < 1e8
        && py.abs() > 1e-8
        && py.abs() < 1e8
        && nn > 1e-12
        && nn < 1e12)
    {
        return whole();
    }
    let n = normal / nn.sqrt();
    let right = view.row(0).truncate();
    let up = view.row(1).truncate();
    let back = view.row(2).truncate();
    let dx = right * (2.0 / (size.x * px));
    let dy = up * (-2.0 / (size.y * py));
    let perspective = projection.w_axis.w.abs() < 0.5;
    let mut ray = -back;
    let mut distance = 1.0;
    if perspective {
        let ndc = pixel / size * Vec2::new(2.0, -2.0) + Vec2::new(-1.0, 1.0);
        ray = right * ((ndc.x + projection.z_axis.x) / px)
            + up * ((ndc.y + projection.z_axis.y) / py)
            - back;
        distance = -(view * world.extend(1.0)).z;
        if !(distance > 0.0 && distance < 1e12) {
            return whole();
        }
    }
    let denominator = n.dot(ray);
    let singular = if perspective {
        1e-5f32.max(n.dot(dx).abs() + n.dot(dy).abs())
    } else {
        1e-5
    };
    if !(denominator.abs() > singular && denominator.abs() < 1e12) {
        return whole();
    }
    let wx = distance * (dx - ray * (n.dot(dx) / denominator));
    let wy = distance * (dy - ray * (n.dot(dy) / denominator));
    let xz = Vec2::new(wx.x, wx.z).abs().min(Vec2::splat(limit))
        + Vec2::new(wy.x, wy.z).abs().min(Vec2::splat(limit));
    Differential {
        dx: wx,
        dy: wy,
        footprint: xz.length().clamp(1e-3, limit),
        whole_field: false,
    }
}
fn double(m: Mat4) -> DMat4 {
    DMat4::from_cols_array(&m.to_cols_array().map(f64::from))
}
// Independent reference: invert the complete jittered view-projection and unproject two
// depth points. No camera basis, P20 sign, analytic differential or candidate helper is used.
fn reference_ray(view: Mat4, projection: Mat4, size: Vec2, pixel: DVec2) -> (DVec3, DVec3) {
    let ndc = pixel / size.as_dvec2() * DVec2::new(2.0, -2.0) + DVec2::new(-1.0, 1.0);
    let inverse = (double(projection) * double(view)).inverse();
    let point = |z| {
        let h = inverse * DVec4::new(ndc.x, ndc.y, z, 1.0);
        h.truncate() / h.w
    };
    let near = point(1.0);
    (near, (point(0.5) - near).normalize())
}
fn plane_hit(
    view: Mat4,
    projection: Mat4,
    size: Vec2,
    pixel: DVec2,
    point: DVec3,
    normal: DVec3,
) -> DVec3 {
    let (origin, ray) = reference_ray(view, projection, size, pixel);
    let denominator = normal.dot(ray);
    assert!(denominator.abs() > 1e-10);
    origin + ray * (normal.dot(point - origin) / denominator)
}
fn footprint(dx: DVec3, dy: DVec3) -> f64 {
    (DVec2::new(dx.x, dx.z).abs() + DVec2::new(dy.x, dy.z).abs())
        .length()
        .max(1e-3)
}

#[test]
fn material_plane_differentials_match_independent_neighbor_ray_intersections() {
    let cameras = [
        CameraState::look_at(Vec3::new(7.0, 8.0, 12.0), Vec3::ZERO),
        CameraState::look_at(Vec3::new(7.0, -8.0, 12.0), Vec3::ZERO),
        CameraState::look_at_up(
            Vec3::new(3.0, 6.0, 9.0),
            Vec3::ZERO,
            Vec3::new(0.3, 1.0, 0.2),
        ),
    ];
    let mut compared = 0;
    for mut camera in cameras {
        for ortho in [false, true] {
            camera.ortho_height = ortho.then_some(24.0);
            let view = camera.view();
            for size in [
                Vec2::new(320.0, 180.0),
                Vec2::new(1280.0, 720.0),
                Vec2::new(4096.0, 2048.0),
            ] {
                for jitter_pixels in [Vec2::ZERO, Vec2::new(0.375, -0.25)] {
                    let jitter = jitter_pixels / size * Vec2::new(2.0, -2.0);
                    let projection =
                        Mat4::from_translation(jitter.extend(0.0)) * camera.proj(size.x / size.y);
                    for uv in [
                        Vec2::new(0.5, 0.5),
                        Vec2::new(0.3, 0.65),
                        Vec2::new(0.75, 0.3),
                    ] {
                        let pixel = uv * size;
                        let (origin, ray) = reference_ray(view, projection, size, pixel.as_dvec2());
                        let point = origin + ray * 20.0;
                        for normal in [
                            Vec3::Y,
                            -Vec3::Y,
                            Vec3::new(0.3, 1.0, -0.2).normalize(),
                            (view.row(2).truncate() + view.row(0).truncate() * 0.5
                                - view.row(1).truncate() * 0.3)
                                .normalize(),
                        ] {
                            let actual = candidate(
                                view,
                                projection,
                                size,
                                pixel,
                                point.as_vec3(),
                                normal,
                                Vec2::splat(128.0),
                            );
                            assert!(
                                !actual.whole_field,
                                "unexpected singular case: ortho={ortho}, size={size}, uv={uv}, normal={normal:?}"
                            );
                            let hit = |offset| {
                                plane_hit(
                                    view,
                                    projection,
                                    size,
                                    pixel.as_dvec2() + offset,
                                    point,
                                    normal.as_dvec3(),
                                )
                            };
                            let epsilon = 0.01;
                            let dx = (hit(DVec2::new(epsilon, 0.0))
                                - hit(DVec2::new(-epsilon, 0.0)))
                                / (epsilon * 2.0);
                            let dy = (hit(DVec2::new(0.0, epsilon))
                                - hit(DVec2::new(0.0, -epsilon)))
                                / (epsilon * 2.0);
                            for (actual, expected) in [(actual.dx, dx), (actual.dy, dy)] {
                                assert!(
                                    (actual.as_dvec3() - expected).length()
                                        <= expected.length() * 0.002 + 2e-6,
                                    "ortho={ortho} size={size} uv={uv} normal={normal:?}: actual={actual:?} expected={expected:?}"
                                );
                            }
                            let expected = footprint(dx, dy);
                            assert!(
                                (f64::from(actual.footprint) - expected).abs()
                                    <= expected * 0.003 + 2e-6
                            );
                            // Actual neighboring camera rays a full pixel apart also agree
                            // for these nonsingular planes, up to finite-pixel perspective curvature.
                            let adjacent_x = hit(DVec2::new(0.5, 0.0)) - hit(DVec2::new(-0.5, 0.0));
                            let adjacent_y = hit(DVec2::new(0.0, 0.5)) - hit(DVec2::new(0.0, -0.5));
                            let adjacent = footprint(adjacent_x, adjacent_y);
                            assert!(
                                (f64::from(actual.footprint) - adjacent).abs()
                                    <= adjacent * 0.025 + 2e-6
                            );
                            compared += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(compared, 432);
    eprintln!(
        "compared {compared} f32 plane differentials against independent f64 neighboring rays"
    );
}

#[test]
fn tangent_footprint_is_normal_sign_invariant_and_ortho_distance_invariant() {
    let mut camera = CameraState::look_at(Vec3::new(7.0, 8.0, 12.0), Vec3::ZERO);
    camera.ortho_height = Some(24.0);
    let view = camera.view();
    let projection = camera.proj(16.0 / 9.0);
    let size = Vec2::new(1280.0, 720.0);
    let pixel = size * Vec2::new(0.3, 0.6);
    let (origin, ray) = reference_ray(view, projection, size, pixel.as_dvec2());
    let run = |distance: f64, normal: Vec3| {
        candidate(
            view,
            projection,
            size,
            pixel,
            (origin + ray * distance).as_vec3(),
            normal,
            Vec2::splat(128.0),
        )
    };
    let a = run(20.0, Vec3::Y);
    let b = run(20.0, -Vec3::Y);
    let c = run(200.0, Vec3::Y);
    assert!(!a.whole_field && !b.whole_field && !c.whole_field);
    assert_eq!(a.footprint.to_bits(), b.footprint.to_bits());
    assert_eq!(a.footprint.to_bits(), c.footprint.to_bits());
    assert_eq!(a.dx, b.dx);
    assert_eq!(a.dy, b.dy);
}

#[test]
fn rounded_scene_extent_and_pre_near_depth_match_independent_rays() {
    let mut camera = CameraState::look_at(Vec3::ZERO, Vec3::new(0.0, -0.5, -1.0));
    let size = Vec2::new(319.0, 179.0);
    let pixel = size * Vec2::new(0.63, 0.42);
    let mut compared = 0;
    for ortho in [false, true] {
        camera.ortho_height = ortho.then_some(24.0);
        let view = camera.view();
        for jitter_pixels in [Vec2::ZERO, Vec2::new(-0.375, 0.25)] {
            let jitter = jitter_pixels / size * Vec2::new(2.0, -2.0);
            // Real render-scale rounding preserves the output camera's projection aspect.
            let projection = Mat4::from_translation(jitter.extend(0.0)) * camera.proj(16.0 / 9.0);
            let (origin, ray) = reference_ray(view, projection, size, pixel.as_dvec2());
            let origin_depth = -(double(view) * origin.extend(1.0)).z;
            let ray_depth = -(double(view) * ray.extend(0.0)).z;
            for depth in [1e-4, 0.05, 20.0] {
                let point = origin + ray * ((depth - origin_depth) / ray_depth);
                let normal = (view.row(2).truncate() + view.row(0).truncate() * 0.3).normalize();
                let actual = candidate(
                    view,
                    projection,
                    size,
                    pixel,
                    point.as_vec3(),
                    normal,
                    Vec2::splat(128.0),
                );
                assert!(!actual.whole_field && actual.footprint.is_finite());
                let hit = |offset| {
                    plane_hit(
                        view,
                        projection,
                        size,
                        pixel.as_dvec2() + offset,
                        point,
                        normal.as_dvec3(),
                    )
                };
                let epsilon = 0.01;
                let dx = (hit(DVec2::new(epsilon, 0.0)) - hit(DVec2::new(-epsilon, 0.0)))
                    / (2.0 * epsilon);
                let dy = (hit(DVec2::new(0.0, epsilon)) - hit(DVec2::new(0.0, -epsilon)))
                    / (2.0 * epsilon);
                for (observed, expected) in [(actual.dx, dx), (actual.dy, dy)] {
                    assert!(
                        (observed.as_dvec3() - expected).length()
                            <= expected.length() * 0.002 + 1e-10,
                        "ortho={ortho} depth={depth}: observed={observed:?}, expected={expected:?}"
                    );
                }
                let expected = footprint(dx, dy);
                assert!((f64::from(actual.footprint) - expected).abs() <= expected * 0.003 + 2e-6);
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 12);
    eprintln!("compared {compared} rounded-scene/output-aspect and pre-near cases");
}

#[test]
fn grazing_and_singular_cases_use_finite_whole_field_without_derivative_fallback() {
    let mut camera = CameraState::look_at(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.0, 2.0, -1.0));
    let size = Vec2::new(1280.0, 720.0);
    let extent = Vec2::new(32.0, 64.0);
    let expected = extent.length();
    for ortho in [false, true] {
        camera.ortho_height = ortho.then_some(24.0);
        let view = camera.view();
        let projection = camera.proj(size.x / size.y);
        for normal in [
            Vec3::Y,
            (Vec3::Y + Vec3::Z * 1e-7).normalize(),
            Vec3::ZERO,
            Vec3::splat(f32::NAN),
        ] {
            let actual = candidate(
                view,
                projection,
                size,
                size * 0.5,
                Vec3::new(0.0, 2.0, -20.0),
                normal,
                extent,
            );
            assert!(actual.whole_field && actual.footprint.is_finite());
            assert!((actual.footprint - expected).abs() < expected * 4.0 * f32::EPSILON);
        }
        let mut singular_projection = projection;
        singular_projection.x_axis.x = 0.0;
        let actual = candidate(
            view,
            singular_projection,
            size,
            size * 0.5,
            Vec3::new(0.0, 2.0, -20.0),
            Vec3::Z,
            extent,
        );
        assert!(actual.whole_field && actual.footprint.is_finite());
    }
    // Perspective tangent horizon within the one-pixel ray fan uses the finite domain.
    let projection = CameraState {
        ortho_height: None,
        ..camera
    }
    .proj(size.x / size.y);
    let actual = candidate(
        camera.view(),
        projection,
        size,
        size * 0.5,
        Vec3::new(0.0, 2.0, -20.0),
        Vec3::new(1.0, 0.0, 1e-4).normalize(),
        extent,
    );
    assert!(actual.whole_field && actual.footprint.is_finite());
}

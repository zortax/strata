use super::*;

use glam::UVec2;
use proptest::prelude::*;

fn test_camera() -> Camera {
    Camera::new(Viewport::new(UVec2::new(1280, 800), 1.0))
}

#[test]
fn project_unproject_round_trip_at_multiple_zooms() {
    let mut camera = test_camera();
    for &zoom in &[MIN_ZOOM, 7.3, 10.0, 12.5, 16.0, MAX_ZOOM] {
        camera.state.zoom = zoom;
        for &(x, y) in &[(0.0, 0.0), (640.0, 400.0), (1280.0, 800.0), (13.7, 791.2)] {
            let screen = DVec2::new(x, y);
            let rt = camera.project(camera.unproject(screen));
            // f64 world coords near 0.36 have ~5e-17 ulps; scaled by
            // 256·2^19 that bounds the round trip at ~1e-8 px. 1e-6 px is
            // the product tolerance (same as the anchor invariant).
            assert!(
                (rt - screen).length() < 1e-6,
                "round trip at zoom {zoom}: {screen:?} → {rt:?}"
            );
        }
        // world → screen → world
        let world = camera.center() + DVec2::new(0.001, -0.0007);
        let rt = camera.unproject(camera.project(world));
        assert!((rt - world).length() < 1e-12);
    }
}

#[test]
fn world_scale_is_256_times_two_to_the_zoom() {
    let mut camera = test_camera();
    camera.state.zoom = 10.0;
    assert_eq!(camera.world_scale(), 256.0 * 1024.0);
}

#[test]
fn zoom_clamps_to_range() {
    let mut camera = test_camera();
    camera.zoom_about(DVec2::new(100.0, 100.0), 100.0);
    assert_eq!(camera.target_zoom(), MAX_ZOOM);
    camera.zoom_about(DVec2::new(100.0, 100.0), -100.0);
    assert_eq!(camera.target_zoom(), MIN_ZOOM);
}

#[test]
fn tick_is_dt_independent() {
    let mut coarse = test_camera();
    let mut fine = coarse.clone();
    let anchor = DVec2::new(900.0, 150.0);
    coarse.zoom_about(anchor, 3.0);
    fine.zoom_about(anchor, 3.0);

    coarse.tick(Duration::from_millis(100));
    for _ in 0..10 {
        fine.tick(Duration::from_millis(10));
    }
    assert!(
        (coarse.zoom() - fine.zoom()).abs() < 1e-9,
        "coarse {} vs fine {}",
        coarse.zoom(),
        fine.zoom()
    );
    assert!((coarse.center() - fine.center()).length() < 1e-12);
}

#[test]
fn zoom_animation_settles_on_target() {
    let mut camera = test_camera();
    camera.zoom_about(DVec2::new(640.0, 400.0), 2.5);
    for _ in 0..600 {
        camera.tick(Duration::from_millis(16));
    }
    assert!(!camera.is_animating());
    assert_eq!(camera.zoom(), 8.5);
}

#[test]
fn fly_to_settles_on_target() {
    let mut camera = test_camera();
    let target = LatLon::new(48.3537, 11.7751); // EDDM
    camera.fly_to(target, 12.0);
    for _ in 0..600 {
        camera.tick(Duration::from_millis(16));
    }
    assert!(!camera.is_animating());
    assert_eq!(camera.zoom(), 12.0);
    let center = crate::geo::lat_lon_from_world(camera.center());
    assert!((center.lat_deg() - target.lat_deg()).abs() < 1e-6);
    assert!((center.lon_deg() - target.lon_deg()).abs() < 1e-6);
}

#[test]
fn pan_during_zoom_keeps_anchor_after_pan() {
    let mut camera = test_camera();
    let anchor = DVec2::new(200.0, 700.0);
    camera.zoom_about(anchor, 4.0);
    camera.tick(Duration::from_millis(16));
    camera.pan_by(DVec2::new(35.0, -12.0));
    // The anchor world point was re-derived; subsequent ticks must hold it.
    let world = camera.unproject(anchor);
    for _ in 0..20 {
        camera.tick(Duration::from_millis(16));
        let err = (camera.project(world) - anchor).length();
        assert!(err < 1e-6, "anchor drifted {err} px after pan");
    }
}

/// True if the center clamp engaged (camera sits exactly on a world edge) —
/// the one situation where the anchor invariant legitimately yields.
fn center_clamped(camera: &Camera) -> bool {
    let c = camera.center();
    c.x == 0.0 || c.x == 1.0 || c.y == 0.0 || c.y == 1.0
}

proptest! {
    /// THE invariant: through arbitrary interleavings of wheel events and
    /// ticks, the world point captured at each `zoom_about` stays under its
    /// anchor screen point to < 1e-6 px for the whole animation.
    #[test]
    fn zoom_anchor_never_drifts(
        events in prop::collection::vec(
            (
                (0.0f64..1280.0, 0.0f64..800.0), // anchor px
                -2.5f64..2.5,                    // zoom delta
                prop::collection::vec(0.001f64..0.05, 1..8), // tick dts (s)
            ),
            1..40,
        )
    ) {
        let mut camera = test_camera();
        for ((ax, ay), zoom_delta, dts) in events {
            let anchor_px = DVec2::new(ax, ay);
            let anchor_world = camera.unproject(anchor_px);
            camera.zoom_about(anchor_px, zoom_delta);
            for dt in dts {
                camera.tick(Duration::from_secs_f64(dt));
                if center_clamped(&camera) {
                    // Edge-of-world clamp overrides the anchor by design.
                    return Ok(());
                }
                let err = (camera.project(anchor_world) - anchor_px).length();
                prop_assert!(
                    err < 1e-6,
                    "anchor drift {err} px at zoom {}", camera.zoom()
                );
            }
        }
    }

    /// Round trip screen → world → screen at random camera poses.
    #[test]
    fn unproject_project_round_trip(
        zoom in MIN_ZOOM..MAX_ZOOM,
        cx in 0.4f64..0.6,
        cy in 0.25f64..0.45,
        px in 0.0f64..1280.0,
        py in 0.0f64..800.0,
    ) {
        let mut camera = test_camera();
        camera.state = CameraState { center: DVec2::new(cx, cy), zoom };
        let screen = DVec2::new(px, py);
        let rt = camera.project(camera.unproject(screen));
        prop_assert!((rt - screen).length() < 1e-6);
    }
}

//! Headless smoke test: real adapter (skipped gracefully if none), build a
//! `MapRenderer`, resize, tick, render frames, and validate every embedded
//! shader on the live device.

use strata_render::glam::{DVec2, UVec2};
use strata_render::gpu::shader::ShaderLibrary;
use strata_render::{
    LayerId, MapInput, MapRenderer, MapTheme, Redraw, RenderRoute, RendererConfig, RoutePointKind,
    RouteVertex, wgpu,
};

use std::sync::Arc;
use std::time::Duration;

fn gpu() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        {
            Ok(adapter) => adapter,
            Err(e) => {
                eprintln!("skipping smoke test: no wgpu adapter available ({e})");
                return None;
            }
        };
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok((device, queue)) => Some((Arc::new(device), Arc::new(queue))),
        Err(e) => {
            eprintln!("skipping smoke test: failed to create device ({e})");
            None
        }
    }
}

#[test]
fn renderer_builds_resizes_ticks_and_renders() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let mut renderer =
        MapRenderer::new(device.clone(), queue, RendererConfig::default()).expect("renderer init");

    renderer.resize(UVec2::new(800, 600), 1.0);
    renderer.input(MapInput::ZoomAbout {
        anchor_px: DVec2::new(400.0, 300.0),
        zoom_delta: 1.5,
    });
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Needed);

    let first = renderer.render();
    assert_eq!(first.width(), 800);
    assert_eq!(first.height(), 600);
    let first_addr = first as *const wgpu::Texture as usize;

    // Ping-pong: consecutive frames land in alternating buffers (the
    // renderer does not move, so the texture addresses identify them).
    let second_addr = renderer.render() as *const wgpu::Texture as usize;
    assert_ne!(second_addr, first_addr);
    let third_addr = renderer.render() as *const wgpu::Texture as usize;
    assert_eq!(third_addr, first_addr);

    // Resize recreates the targets at the new physical size.
    renderer.resize(UVec2::new(1024, 512), 2.0);
    let resized = renderer.render();
    assert_eq!(resized.width(), 1024);
    assert_eq!(resized.height(), 512);

    // Layer toggles round-trip; disabled layers are skipped, not drawn.
    renderer.set_layer_enabled(LayerId::Weather, false);
    assert!(!renderer.layer_enabled(LayerId::Weather));
    assert!(renderer.layer_enabled(LayerId::Basemap));
    // The gridded weather overlays are opt-in (default off).
    for id in [
        LayerId::CloudCover,
        LayerId::Precipitation,
        LayerId::Thunderstorms,
    ] {
        assert!(!renderer.layer_enabled(id), "{id:?} must default off");
    }
    renderer.set_layer_enabled(LayerId::Precipitation, true);
    assert!(renderer.layer_enabled(LayerId::Precipitation));
    renderer.render();

    // Camera snapshot is geographically sane (Germany default view).
    let snapshot = renderer.camera();
    assert!(snapshot.zoom > 6.0, "zoom animated past start");
    let (sw, ne) = snapshot.bounds;
    assert!(sw.lat_deg() < snapshot.center.lat_deg());
    assert!(ne.lat_deg() > snapshot.center.lat_deg());
    assert!(sw.lon_deg() < ne.lon_deg());

    // Settle the animation to idle.
    for _ in 0..1000 {
        if renderer.tick(Duration::from_millis(16)) == Redraw::Idle {
            break;
        }
        renderer.render();
    }
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Idle);

    // The route layer is always enabled (no user toggle) but draws nothing
    // until a route is set; setting one demands a redraw, an identical
    // re-push keeps the renderer idle.
    assert!(renderer.layer_enabled(LayerId::Route));
    assert!(renderer.route().is_none());
    let route = RenderRoute {
        points: vec![
            RouteVertex {
                id: 1,
                pos: [8.64, 49.96],
                kind: RoutePointKind::Departure,
            },
            RouteVertex {
                id: 2,
                pos: [11.2, 49.9],
                kind: RoutePointKind::Destination,
            },
        ],
        leg_conflict: vec![false],
        ..RenderRoute::default()
    };
    renderer.set_route(Some(route.clone()));
    assert_eq!(renderer.route(), Some(&route));
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Needed);
    for _ in 0..1000 {
        if renderer.tick(Duration::from_millis(16)) == Redraw::Idle {
            break;
        }
        renderer.render();
    }
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Idle);
    renderer.set_route(Some(route));
    assert_eq!(
        renderer.tick(Duration::from_millis(16)),
        Redraw::Idle,
        "identical route must keep the renderer idle"
    );
    renderer.set_route(None);
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Needed);
    renderer.render();

    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
}

/// `set_map_theme` must bump the style generation (everything
/// color-dependent regenerates), update the clear color, keep rendering,
/// and treat an identical theme as a no-op.
#[test]
fn map_theme_switch_bumps_the_style_generation_and_keeps_rendering() {
    let Some((device, queue)) = gpu() else {
        return;
    };
    let mut renderer =
        MapRenderer::new(device.clone(), queue, RendererConfig::default()).expect("renderer init");
    renderer.resize(UVec2::new(320, 240), 1.0);
    renderer.render();

    assert_eq!(renderer.map_theme().id, "oldworld", "default theme");
    assert_eq!(renderer.style_generation(), 0);

    // Re-applying the active theme is a no-op (no needless regeneration).
    renderer.set_map_theme(MapTheme::oldworld());
    assert_eq!(renderer.style_generation(), 0, "identical theme is a no-op");

    renderer.set_map_theme(MapTheme::pastel_light());
    assert_eq!(renderer.style_generation(), 1, "theme switch regenerates");
    assert_eq!(renderer.map_theme().id, "pastel-light");
    // The switch marks the renderer dirty: the next tick demands a redraw.
    assert_eq!(renderer.tick(Duration::from_millis(16)), Redraw::Needed);
    renderer.render();

    renderer.set_map_theme(MapTheme::high_contrast());
    assert_eq!(renderer.style_generation(), 2);
    renderer.render();

    // Built-in lookup round-trips through the renderer.
    let by_id = MapTheme::by_id("high-contrast").expect("built-in id");
    assert_eq!(renderer.map_theme(), &by_id);

    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("device poll");
}

#[test]
fn embedded_shaders_compile_on_device() {
    let Some((device, _queue)) = gpu() else {
        return;
    };
    let library = ShaderLibrary::embedded();
    for name in library.names().collect::<Vec<_>>() {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = library
            .create_module(&device, name)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("{name} failed device validation: {error}");
        }
    }
}

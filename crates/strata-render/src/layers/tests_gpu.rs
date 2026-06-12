//! Live-device tests for the aero layers: prepare (worker tessellation +
//! pipeline creation) and draw against a real adapter, so vertex-layout or
//! bind-group mismatches fail here instead of at first app launch. Skipped
//! gracefully when no GPU is available (mirrors `tests/smoke.rs`).

use crate::camera::{Camera, Viewport};
use crate::features::{
    AirspaceStyleKey, FlightCategoryColor, GriddedField, PointKind, RenderAirspace,
    RenderPointFeature, RenderRoute, RenderSigmet, RoutePointKind, RouteVertex, WeatherGridFrame,
};
use crate::geo::LatLon;
use crate::gpu::shader::ShaderLibrary;
use crate::gpu::{GLOBALS_BIND_GROUP_INDEX, Globals};
use crate::layer::{DrawCtx, FrameInfo, LayerToggles, MapLayer, PrepareCtx};
use crate::layers::{AirspaceLayer, GriddedWeatherLayer, PointLayer, RouteLayer, WeatherLayer};
use crate::workers::WorkerPool;

use glam::UVec2;

use std::time::{Duration, Instant};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        {
            Ok(adapter) => adapter,
            Err(e) => {
                eprintln!("skipping aero-layer GPU test: no adapter ({e})");
                return None;
            }
        };
    match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())) {
        Ok(pair) => Some(pair),
        Err(e) => {
            eprintln!("skipping aero-layer GPU test: no device ({e})");
            None
        }
    }
}

struct Harness {
    device: wgpu::Device,
    queue: wgpu::Queue,
    camera: Camera,
    workers: WorkerPool,
    toggles: LayerToggles,
    globals: Globals,
    shaders: ShaderLibrary,
    target: wgpu::Texture,
}

impl Harness {
    fn new() -> Option<Self> {
        let (device, queue) = gpu()?;
        let mut camera = Camera::new(Viewport::new(UVec2::new(640, 480), 1.0));
        // Settle the zoom animation so labels/symbols at Germany are in view.
        camera.tick(Duration::from_secs(10));
        let globals = Globals::new(&device);
        globals.update(&queue, &camera);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aero layer test target"),
            size: wgpu::Extent3d {
                width: 640,
                height: 480,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        Some(Self {
            device,
            queue,
            camera,
            workers: WorkerPool::new(2),
            toggles: LayerToggles::all_enabled(),
            globals,
            shaders: ShaderLibrary::embedded(),
            target,
        })
    }

    /// A single prepare pass (for tests that observe intermediate states).
    fn prepare_once(&self, layer: &mut dyn MapLayer) {
        let mut ctx = PrepareCtx {
            device: &self.device,
            queue: &self.queue,
            camera: &self.camera,
            workers: &self.workers,
            layers: &self.toggles,
            frame: FrameInfo {
                dt: Duration::from_millis(16),
                frame_index: 0,
            },
            target_format: FORMAT,
            globals_layout: &self.globals.layout,
            shaders: &self.shaders,
        };
        layer.prepare(&mut ctx);
    }

    /// Prepare until the layer's worker results landed (bounded wait).
    fn prepare_until_settled(&self, layer: &mut dyn MapLayer) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frame_index = 0;
        loop {
            let mut ctx = PrepareCtx {
                device: &self.device,
                queue: &self.queue,
                camera: &self.camera,
                workers: &self.workers,
                layers: &self.toggles,
                frame: FrameInfo {
                    dt: Duration::from_millis(16),
                    frame_index,
                },
                target_format: FORMAT,
                globals_layout: &self.globals.layout,
                shaders: &self.shaders,
            };
            layer.prepare(&mut ctx);
            frame_index += 1;
            if !layer.wants_redraw() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "worker results never arrived (wants_redraw stuck)"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Draw the layer into the offscreen target inside a validation scope.
    fn draw_validated(&self, layer: &dyn MapLayer, label: &str) {
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let view = self.target.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(GLOBALS_BIND_GROUP_INDEX, &self.globals.bind_group, &[]);
            let ctx = DrawCtx {
                camera: &self.camera,
                layers: &self.toggles,
                globals: &self.globals.bind_group,
            };
            layer.draw(&mut pass, &ctx);
        }
        self.queue.submit([encoder.finish()]);
        if let Some(error) = pollster::block_on(scope.pop()) {
            panic!("{label}: GPU validation failed: {error}");
        }
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("device poll");
    }
}

#[test]
fn airspace_layer_prepares_and_draws_on_gpu() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let mut layer = AirspaceLayer::default();
    layer.set_airspaces(vec![
        RenderAirspace {
            id: 1,
            style: AirspaceStyleKey::Ctr,
            polygon: vec![[10.0, 50.0], [10.3, 50.0], [10.3, 50.2], [10.0, 50.2]],
            holes: vec![vec![
                [10.1, 50.05],
                [10.2, 50.05],
                [10.2, 50.1],
                [10.1, 50.1],
            ]],
            lower_label: "GND".into(),
            upper_label: "2500 MSL".into(),
            name: "TEST CTR".into(),
        },
        RenderAirspace {
            id: 2,
            style: AirspaceStyleKey::Tmz,
            polygon: vec![[9.0, 49.0], [9.5, 49.0], [9.5, 49.4], [9.0, 49.4]],
            holes: vec![],
            lower_label: "1000 AGL".into(),
            upper_label: "FL 100".into(),
            name: "TEST TMZ".into(),
        },
    ]);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.labels().len(), 2, "one band label per airspace");
    harness.draw_validated(&layer, "airspace gpu draw");
}

#[test]
fn point_layer_prepares_and_draws_on_gpu() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let mut layer = PointLayer::default();
    layer.set_points(vec![
        RenderPointFeature {
            id: 1,
            kind: PointKind::AirportIntl,
            position: LatLon::new(50.03, 8.57),
            label: Some("EDDF".into()),
            rotation_deg: Some(67.0), // EDDF 07/25 runway heading
        },
        RenderPointFeature {
            id: 2,
            kind: PointKind::Vor,
            position: LatLon::new(50.05, 8.64),
            label: Some("FFM 114.2".into()),
            rotation_deg: None,
        },
        RenderPointFeature {
            id: 3,
            kind: PointKind::Obstacle,
            position: LatLon::new(50.1, 8.7),
            label: None,
            rotation_deg: None,
        },
        RenderPointFeature {
            id: 4,
            kind: PointKind::WeatherStation(FlightCategoryColor::Mvfr),
            position: LatLon::new(50.03, 8.57),
            label: None,
            rotation_deg: None,
        },
    ]);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.visible_labels(&harness.toggles).count(), 2);
    harness.draw_validated(&layer, "point gpu draw");
}

/// The route layer's two pipelines (route_line.wgsl strokes + symbol.wgsl
/// marker instances) prepare and draw against a real device — vertex-layout
/// or bind-group mismatches fail here instead of at first app launch. Also
/// exercises the scrub-only refresh path on live buffers.
#[test]
fn route_layer_prepares_and_draws_on_gpu() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let mut layer = RouteLayer::default();
    let route = RenderRoute {
        points: vec![
            RouteVertex {
                id: 1,
                pos: [8.64, 49.96],
                kind: RoutePointKind::Departure,
            },
            RouteVertex {
                id: 2,
                pos: [9.5, 50.2],
                kind: RoutePointKind::Waypoint,
            },
            RouteVertex {
                id: 3,
                pos: [11.2, 49.9],
                kind: RoutePointKind::Destination,
            },
            RouteVertex {
                id: 4,
                pos: [11.8, 49.5],
                kind: RoutePointKind::Alternate,
            },
        ],
        leg_conflict: vec![false, true],
        leg_labels: vec![
            Some("MH 062 · 118 kt · 4500".to_owned()),
            Some("MH 102 · 121 kt · 4500".to_owned()),
        ],
        toc: Some((20_000.0, [8.85, 50.03])),
        tod: Some((150_000.0, [10.9, 49.95])),
        corridor_halfwidth_m: Some(4_000.0),
        scrub_along_m: Some(60_000.0),
        ..RenderRoute::default()
    };
    assert!(layer.set_route(Some(route.clone())));
    // Frame-fresh contract: a realistically sized route tessellates
    // synchronously, so a single prepare — the one following a drag's
    // set_route — already settles the new geometry (no worker round-trip,
    // no extra frames demanded).
    harness.prepare_once(&mut layer);
    assert!(
        !layer.wants_redraw(),
        "small routes must build synchronously within one prepare"
    );
    assert_eq!(layer.labels().len(), 2, "one label per labelled leg");
    harness.draw_validated(&layer, "route gpu draw");

    // A geometry change (the drag's per-move push) is again fresh after
    // one prepare.
    let mut moved = route;
    moved.points[1].pos = [9.6, 50.25];
    assert!(layer.set_route(Some(moved.clone())));
    harness.prepare_once(&mut layer);
    assert!(
        !layer.wants_redraw(),
        "a moved waypoint must re-tessellate within the same prepare"
    );
    harness.draw_validated(&layer, "route gpu draw after waypoint move");

    // Scrub-only move: no worker rebuild (stays settled), still draws.
    let mut scrubbed = moved;
    scrubbed.scrub_along_m = Some(90_000.0);
    assert!(layer.set_route(Some(scrubbed.clone())));
    harness.prepare_once(&mut layer);
    assert!(
        !layer.wants_redraw(),
        "scrub must not start a worker rebuild"
    );
    harness.draw_validated(&layer, "route gpu draw after scrub");

    // Snap ring on: the ring uniform/pipeline draw validates and the layer
    // animates (wants_redraw) without any worker rebuild.
    let mut snapped = scrubbed;
    snapped.snap_ring = Some([9.5, 50.2]);
    assert!(layer.set_route(Some(snapped)));
    harness.prepare_once(&mut layer);
    assert!(
        layer.wants_redraw(),
        "an active snap ring keeps the pulse animating"
    );
    harness.draw_validated(&layer, "route gpu draw with snap ring");

    // Clearing the route empties the layer; the draw becomes a no-op.
    assert!(layer.set_route(None));
    harness.prepare_once(&mut layer);
    assert!(layer.labels().is_empty(), "cleared route keeps no labels");
    assert!(!layer.wants_redraw(), "cleared route stops the ring pulse");
    harness.draw_validated(&layer, "route gpu draw after clear");
}

#[test]
fn weather_layer_prepares_and_draws_on_gpu() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let mut layer = WeatherLayer::default();
    layer.set_sigmets(vec![RenderSigmet {
        polygon: vec![[8.0, 49.0], [11.0, 49.0], [11.0, 51.0], [8.0, 51.0]],
        hazard_label: "SEV TURB".into(),
    }]);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.labels().len(), 1);
    harness.draw_validated(&layer, "weather gpu draw");
}

/// The gridded weather layer converts frames on the workers, uploads the
/// bracketing pair as `Rg16Float` textures and draws blended quads for
/// every enabled field — exercised against a real device so bind-group or
/// uniform-layout mismatches fail here.
#[test]
fn gridded_weather_layer_prepares_and_draws_on_gpu() {
    fn frame(field: GriddedField, valid_time: i64) -> WeatherGridFrame {
        WeatherGridFrame {
            field,
            valid_time,
            extent: (47.0, 6.0, 55.0, 15.0),
            ni: 4,
            nj: 3,
            values: vec![
                0.0,
                2.5,
                f32::NAN,
                10.0,
                1.0,
                0.4,
                30.0,
                f32::NAN,
                0.0,
                5.0,
                60.0,
                100.0,
            ],
        }
    }

    let Some(harness) = Harness::new() else {
        return;
    };
    let mut layer = GriddedWeatherLayer::default();
    for field in GriddedField::ALL {
        layer.set_frames(field, vec![frame(field, 1000), frame(field, 2000)]);
    }
    layer.set_time(1500); // between the frames: both textures must bind
    harness.prepare_until_settled(&mut layer);
    for field in GriddedField::ALL {
        assert!(layer.has_drawable(field), "{field:?} must be drawable");
    }
    harness.draw_validated(&layer, "gridded weather gpu draw");

    // Scrubbing to a cached time stays ready without new uploads.
    layer.set_time(2000);
    harness.prepare_until_settled(&mut layer);
    assert!(layer.has_drawable(GriddedField::PrecipRate));
    harness.draw_validated(&layer, "gridded weather gpu draw (range end)");
}

/// A frame with NaN no-data holes, like every real DWD/radar frame — the
/// texture-stability tests must run on NaN-bearing data because `f32`
/// equality treats NaN-identical pushes as different.
fn nan_frame(field: GriddedField, valid_time: i64, value: f32) -> WeatherGridFrame {
    WeatherGridFrame {
        field,
        valid_time,
        extent: (47.0, 6.0, 55.0, 15.0),
        ni: 3,
        nj: 2,
        values: vec![value, f32::NAN, value * 2.0, 0.0, value, f32::NAN],
    }
}

/// Pushing a superset (the fetch scheduler re-pushes the full frame list
/// after every arrival) must neither drop the bound draw pair nor
/// re-upload textures that are already resident — the enable-time flicker
/// was exactly this: every arrival nuked and re-uploaded the bracket pair.
#[test]
fn gridded_weather_superset_push_reuses_textures() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let field = GriddedField::CloudCover;
    let mut layer = GriddedWeatherLayer::default();
    layer.set_frames(
        field,
        vec![nan_frame(field, 1000, 1.0), nan_frame(field, 2000, 2.0)],
    );
    layer.set_time(1500);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.current_pair(field), Some((1000, 2000)));
    assert_eq!(layer.upload_count(field), 2);

    // An identical refresh (NaN holes included) is a complete no-op.
    assert!(!layer.set_frames(
        field,
        vec![nan_frame(field, 1000, 1.0), nan_frame(field, 2000, 2.0)]
    ));

    // A superset push keeps the draw alive and re-uploads nothing.
    assert!(layer.set_frames(
        field,
        vec![
            nan_frame(field, 1000, 1.0),
            nan_frame(field, 2000, 2.0),
            nan_frame(field, 3000, 3.0),
            nan_frame(field, 4000, 4.0),
        ],
    ));
    assert!(
        layer.has_drawable(field),
        "working-set growth must not drop the bound pair"
    );
    harness.prepare_until_settled(&mut layer);
    assert_eq!(
        layer.upload_count(field),
        2,
        "resident textures are reused; off-bracket frames stay lazy"
    );
    assert_eq!(layer.current_pair(field), Some((1000, 2000)));
    harness.draw_validated(&layer, "gridded weather superset draw");

    // Scrubbing onward converts only the newly needed frame.
    layer.set_time(2500);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.current_pair(field), Some((2000, 3000)));
    assert_eq!(layer.upload_count(field), 3);
    let displayed = layer.displayed_fraction(field).expect("drawable");
    assert!(
        (displayed - 0.5).abs() < 1e-4,
        "settled mid-blend, got {displayed}"
    );
}

/// New data for an existing valid time (a fresher model run / radar
/// refresh) swaps the texture atomically: the old pair keeps drawing
/// until the replacement is resident — never a blank frame — and only the
/// changed frame converts again.
#[test]
fn gridded_weather_replace_with_new_data_swaps_atomically() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let field = GriddedField::PrecipRate;
    let mut layer = GriddedWeatherLayer::default();
    layer.set_frames(
        field,
        vec![nan_frame(field, 1000, 1.0), nan_frame(field, 2000, 2.0)],
    );
    layer.set_time(1500);
    harness.prepare_until_settled(&mut layer);
    let before = layer.current_contents(field).expect("drawable");

    assert!(layer.set_frames(
        field,
        vec![nan_frame(field, 1000, 1.0), nan_frame(field, 2000, 9.0)],
    ));
    assert!(
        layer.has_drawable(field),
        "the stale pair must keep drawing until the replacement is resident"
    );
    harness.draw_validated(&layer, "gridded weather draw during pending swap");

    harness.prepare_until_settled(&mut layer);
    let after = layer.current_contents(field).expect("drawable");
    assert_eq!(
        layer.upload_count(field),
        3,
        "only the changed frame re-converted"
    );
    assert_eq!(after.0, before.0, "unchanged frame kept its texture");
    assert_ne!(
        after.1, before.1,
        "changed frame swapped to the new content"
    );
    assert_eq!(layer.current_pair(field), Some((1000, 2000)));
    harness.draw_validated(&layer, "gridded weather draw after swap");
}

/// Held at the cached frontier, then the missing frame arrives: the blend
/// eases from the held frame into the correct fraction instead of
/// snapping ('stop then jump-fade' becomes a gentle catch-up).
#[test]
fn gridded_weather_hold_then_arrival_ramps_instead_of_snapping() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let field = GriddedField::CloudCover;
    let mut layer = GriddedWeatherLayer::default();
    layer.set_frames(field, vec![nan_frame(field, 1000, 1.0)]);
    layer.set_time(2000); // past the frontier: hold the single frame
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.current_pair(field), Some((1000, 1000)));
    assert_eq!(layer.displayed_fraction(field), Some(0.0));
    assert!(
        !layer.wants_redraw(),
        "a held frame is settled, not animating"
    );

    layer.set_frames(
        field,
        vec![nan_frame(field, 1000, 1.0), nan_frame(field, 3000, 3.0)],
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.prepare_once(&mut layer);
        if layer.current_pair(field) == Some((1000, 3000)) {
            break;
        }
        assert!(Instant::now() < deadline, "arrival never rebound the pair");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(layer.wants_redraw(), "re-blend ramp is animating");
    assert_eq!(
        layer.displayed_fraction(field),
        Some(0.0),
        "the ramp starts at the held frame, not at the target blend"
    );
    harness.prepare_until_settled(&mut layer);
    let displayed = layer.displayed_fraction(field).expect("drawable");
    assert!(
        (displayed - 0.5).abs() < 1e-4,
        "ramp eased into the correct blend, got {displayed}"
    );
    assert!(!layer.wants_redraw(), "settled after the ramp");
}

/// A far scrub past the frontier: the arriving bracket does not contain
/// the held frame at all. A transitional crossfade bridges from the held
/// texture to the nearer new frame, then the real pair ramps in — no
/// hard cut.
#[test]
fn gridded_weather_far_arrival_crossfades_from_held_frame() {
    let Some(harness) = Harness::new() else {
        return;
    };
    let field = GriddedField::CloudCover;
    let mut layer = GriddedWeatherLayer::default();
    layer.set_frames(field, vec![nan_frame(field, 1000, 1.0)]);
    layer.set_time(5000);
    harness.prepare_until_settled(&mut layer);
    assert_eq!(layer.current_pair(field), Some((1000, 1000)));

    layer.set_frames(
        field,
        vec![
            nan_frame(field, 1000, 1.0),
            nan_frame(field, 4900, 2.0),
            nan_frame(field, 5100, 3.0),
        ],
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.prepare_once(&mut layer);
        if layer.current_pair(field) != Some((1000, 1000)) {
            break;
        }
        assert!(Instant::now() < deadline, "arrival never rebound the pair");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        layer.current_pair(field),
        Some((1000, 5100)),
        "crossfade bridge from the held frame to the nearer new frame"
    );
    assert_eq!(layer.displayed_fraction(field), Some(0.0));
    assert!(layer.wants_redraw());
    harness.draw_validated(&layer, "gridded weather transitional crossfade draw");

    harness.prepare_until_settled(&mut layer);
    assert_eq!(
        layer.current_pair(field),
        Some((4900, 5100)),
        "real pair bound after the crossfade"
    );
    let displayed = layer.displayed_fraction(field).expect("drawable");
    assert!(
        (displayed - 0.5).abs() < 1e-4,
        "eased into the correct blend, got {displayed}"
    );
    assert!(!layer.wants_redraw());
}

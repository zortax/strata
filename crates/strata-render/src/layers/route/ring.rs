//! The snap-indication ring: pulse animation math and the GPU plumbing
//! (uniform + bind group) for `route_ring.wgsl`.
//!
//! Same animation model as the basemap tile fade: the layer accumulates
//! frame `dt` into a phase while a ring is shown, derives this frame's
//! pulse values on the CPU and re-writes one small uniform — the layer
//! reports redraw demand for as long as the ring is visible and goes
//! fully idle the moment it clears.

use bytemuck::{Pod, Zeroable};
use glam::DVec2;

use std::time::Duration;

/// One pulse cycle (expand + fade) in seconds.
pub const PULSE_PERIOD_S: f32 = 1.1;
/// Pulse radius sweep in logical px: starts snug around a waypoint handle,
/// expands past the snap radius (12 px in the app) and fades out.
pub const PULSE_RADIUS_PX: (f32, f32) = (7.0, 16.0);
/// Ring band thickness in logical px.
pub const RING_THICKNESS_PX: f32 = 2.0;
/// Peak ring opacity — deliberately subtle; the route accent color keeps
/// it readable.
pub const PULSE_PEAK_ALPHA: f32 = 0.85;

/// Gap between a hover-highlighted handle's (enlarged) edge and its glow
/// ring, logical px.
pub const HIGHLIGHT_RING_GAP_PX: f32 = 2.5;
/// Hover-glow ring band thickness in logical px — wider and softer than
/// the snap ring's 2 px band (it is a static glow, not an announcement).
pub const HIGHLIGHT_RING_THICKNESS_PX: f32 = 3.0;
/// Hover-glow ring opacity. Static — deliberately calmer than the snap
/// ring's pulse peak so the two never read as the same affordance.
pub const HIGHLIGHT_RING_ALPHA: f32 = 0.55;

/// This frame's pulse: ring radius in logical px and the fade factor to
/// premultiply into the ring color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pulse {
    pub radius_px: f32,
    pub fade: f32,
}

/// Sonar-style pulse at `phase` seconds into the animation: each cycle the
/// radius eases outward while the opacity falls to zero, then the pulse
/// restarts. Continuous in `phase` except at the (invisible, zero-alpha)
/// cycle wrap.
pub fn pulse(phase: f32) -> Pulse {
    let t = (phase / PULSE_PERIOD_S).rem_euclid(1.0);
    // Ease-out keeps the expansion lively at the start and gentle at the
    // (faded) end.
    let eased = 1.0 - (1.0 - t) * (1.0 - t);
    let (r0, r1) = PULSE_RADIUS_PX;
    Pulse {
        radius_px: r0 + (r1 - r0) * eased,
        fade: PULSE_PEAK_ALPHA * (1.0 - t),
    }
}

/// Phase accumulator for the pulse, restarted whenever the snap target
/// appears or moves (each new target announces itself with a fresh pulse).
#[derive(Debug, Default)]
pub struct RingAnimation {
    phase: f32,
    target: Option<[f64; 2]>,
}

impl RingAnimation {
    /// Advance one frame toward `target` (`None` clears the ring). Returns
    /// the pulse to draw, or `None` when no ring is shown.
    pub fn advance(&mut self, target: Option<[f64; 2]>, dt: Duration) -> Option<Pulse> {
        if target != self.target {
            self.phase = 0.0;
            self.target = target;
        } else if target.is_some() {
            // Accumulate modulo the period so a long-held snap never loses
            // float precision.
            self.phase = (self.phase + dt.as_secs_f32()).rem_euclid(PULSE_PERIOD_S);
        }
        self.target.map(|_| pulse(self.phase))
    }

    /// Whether a ring is currently shown (the layer then wants redraws).
    pub fn active(&self) -> bool {
        self.target.is_some()
    }
}

/// CPU mirror of the `RingLocals` uniform in `route_ring.wgsl`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct RingUniform {
    /// Ring center in camera-relative world units.
    pub center_rel: [f32; 2],
    /// Current pulse radius, logical px.
    pub radius_px: f32,
    /// Band thickness, logical px.
    pub thickness_px: f32,
    /// Premultiplied ring color with the pulse fade applied.
    pub color: [f32; 4],
}

impl RingUniform {
    /// Assemble this frame's uniform: the camera-relative center (f64
    /// subtraction done by the caller), the pulse and the theme's route
    /// accent with the fade premultiplied onto all four channels.
    pub fn new(center_rel: DVec2, pulse: Pulse, line_color: [f32; 4]) -> Self {
        Self {
            center_rel: [center_rel.x as f32, center_rel.y as f32],
            radius_px: pulse.radius_px,
            thickness_px: RING_THICKNESS_PX,
            color: line_color.map(|c| c * pulse.fade),
        }
    }

    /// The hover highlight's static glow ring around a handle drawn at
    /// `handle_size_px` (the enlarged size): theme accent at a fixed,
    /// calm opacity, sized to clear the handle by the authored gap. No
    /// animation — the uniform only moves with the camera.
    pub fn highlight(center_rel: DVec2, handle_size_px: f32, line_color: [f32; 4]) -> Self {
        Self {
            center_rel: [center_rel.x as f32, center_rel.y as f32],
            radius_px: handle_size_px + HIGHLIGHT_RING_GAP_PX,
            thickness_px: HIGHLIGHT_RING_THICKNESS_PX,
            color: line_color.map(|c| c * HIGHLIGHT_RING_ALPHA),
        }
    }
}

/// The ring uniform buffer + bind group (group 1 of `route_ring.wgsl`).
/// Mirrors [`crate::layers::pipelines::OriginBinding`], but the uniform is
/// also read by the fragment stage (radius/thickness/color).
pub struct RingBinding {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    buffer: wgpu::Buffer,
}

impl RingBinding {
    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<RingUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<RingUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        Self {
            layout,
            bind_group,
            buffer,
        }
    }

    /// Write this frame's uniform.
    pub fn update(&self, queue: &wgpu::Queue, uniform: &RingUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(uniform));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pulse stays inside its authored envelope over a whole cycle and
    /// fades to (near) zero exactly where the radius peaks — the sonar
    /// shape: expand while fading.
    #[test]
    fn pulse_expands_while_fading() {
        let (r0, r1) = PULSE_RADIUS_PX;
        let mut last_radius = f32::NEG_INFINITY;
        let mut last_fade = f32::INFINITY;
        let steps = 64;
        for i in 0..steps {
            let phase = PULSE_PERIOD_S * (i as f32 + 0.001) / steps as f32;
            let p = pulse(phase);
            assert!((r0..=r1).contains(&p.radius_px), "radius {p:?}");
            assert!((0.0..=PULSE_PEAK_ALPHA).contains(&p.fade), "fade {p:?}");
            assert!(p.radius_px >= last_radius, "radius must grow monotonically");
            assert!(p.fade <= last_fade, "fade must fall monotonically");
            last_radius = p.radius_px;
            last_fade = p.fade;
        }
        // Cycle start: smallest, brightest. Cycle end: largest, invisible.
        assert_eq!(pulse(0.0).radius_px, r0);
        assert_eq!(pulse(0.0).fade, PULSE_PEAK_ALPHA);
        let end = pulse(PULSE_PERIOD_S - 1e-4);
        assert!(
            end.fade < 0.01,
            "pulse must fade out before the wrap: {end:?}"
        );
        // The wrap restarts the cycle (phase is periodic).
        let wrapped = pulse(PULSE_PERIOD_S + 0.2);
        let restarted = pulse(0.2);
        assert!((wrapped.radius_px - restarted.radius_px).abs() < 1e-3);
        assert!((wrapped.fade - restarted.fade).abs() < 1e-3);
    }

    /// The animation restarts its phase whenever the target appears or
    /// moves, accumulates dt while it holds, and goes idle on `None`.
    #[test]
    fn animation_restarts_per_target_and_idles_without_one() {
        let mut anim = RingAnimation::default();
        assert!(!anim.active());
        assert_eq!(anim.advance(None, Duration::from_millis(16)), None);
        assert!(!anim.active());

        let a = Some([10.0, 50.0]);
        let first = anim.advance(a, Duration::from_millis(16)).expect("ring");
        assert!(anim.active());
        assert_eq!(first, pulse(0.0), "a new target starts a fresh pulse");

        let held = anim.advance(a, Duration::from_millis(100)).expect("ring");
        assert_eq!(held, pulse(0.1), "holding the target advances the phase");

        // A different target restarts the pulse.
        let b = Some([10.1, 50.0]);
        let moved = anim.advance(b, Duration::from_millis(16)).expect("ring");
        assert_eq!(moved, pulse(0.0));

        // Clearing stops the animation; re-snapping restarts it.
        assert_eq!(anim.advance(None, Duration::from_millis(16)), None);
        assert!(!anim.active());
        let again = anim.advance(b, Duration::from_millis(16)).expect("ring");
        assert_eq!(again, pulse(0.0));
    }

    /// A long-held snap keeps the phase bounded (no float-precision drift).
    #[test]
    fn long_holds_keep_the_phase_bounded() {
        let mut anim = RingAnimation::default();
        let target = Some([10.0, 50.0]);
        anim.advance(target, Duration::ZERO);
        for _ in 0..10_000 {
            anim.advance(target, Duration::from_millis(16));
        }
        let p = anim.advance(target, Duration::ZERO).expect("ring");
        assert!(p.radius_px.is_finite() && p.fade.is_finite());
        let (r0, r1) = PULSE_RADIUS_PX;
        assert!((r0..=r1).contains(&p.radius_px));
    }

    /// Uniform assembly: camera-relative center, pulse values and the
    /// premultiplied fade on every color channel.
    #[test]
    fn uniform_premultiplies_the_fade() {
        let line = [0.8, 0.4, 0.2, 1.0];
        let p = Pulse {
            radius_px: 11.0,
            fade: 0.5,
        };
        let u = RingUniform::new(DVec2::new(0.25, -0.5), p, line);
        assert_eq!(u.center_rel, [0.25, -0.5]);
        assert_eq!(u.radius_px, 11.0);
        assert_eq!(u.thickness_px, RING_THICKNESS_PX);
        assert_eq!(u.color, [0.4, 0.2, 0.1, 0.5]);
        // Premultiplication invariant survives the fade.
        for c in &u.color[..3] {
            assert!(*c <= u.color[3] + 1e-6);
        }
    }
}

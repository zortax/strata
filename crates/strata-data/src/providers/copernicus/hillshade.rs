//! Pure hillshade math: Horn gradient → Lambertian shade with a subtle
//! slope darkening. No IO, no raster handling.
//!
//! Axis conventions used throughout: `x` grows east, `y` grows north,
//! `z` (elevation) grows up. Elevation windows are `window[row][col]` with
//! row 0 the northernmost row and col 0 the westernmost column.

/// Fixed chart-style illumination: sun from the north-west.
pub(crate) const SUN_AZIMUTH_DEG: f64 = 315.0;
pub(crate) const SUN_ALTITUDE_DEG: f64 = 45.0;

/// Fraction of brightness removed at [`SLOPE_DARKEN_FULL_AT_RAD`] —
/// the "subtle slope darkening" on top of plain Lambertian shade.
const SLOPE_DARKEN_STRENGTH: f64 = 0.30;
const SLOPE_DARKEN_FULL_AT_RAD: f64 = std::f64::consts::FRAC_PI_4;

/// Horn (third-order finite difference) gradient of a 3×3 elevation window.
/// Returns `(dz/dx, dz/dy)` in m/m with `x` east and `y` north; `dx_m`/`dy_m`
/// are the ground distances between adjacent samples.
pub(crate) fn horn_gradient(window: &[[f64; 3]; 3], dx_m: f64, dy_m: f64) -> (f64, f64) {
    let w = window;
    let dzdx = ((w[0][2] + 2.0 * w[1][2] + w[2][2]) - (w[0][0] + 2.0 * w[1][0] + w[2][0]))
        / (8.0 * dx_m);
    // North is row 0, so the northward derivative is (north row − south row).
    let dzdy = ((w[0][0] + 2.0 * w[0][1] + w[0][2]) - (w[2][0] + 2.0 * w[2][1] + w[2][2]))
        / (8.0 * dy_m);
    (dzdx, dzdy)
}

/// Lambertian shade in `0..=1` for a surface with gradient
/// `(dzdx_east, dzdy_north)` lit from compass `azimuth_deg` at
/// `altitude_deg` above the horizon.
///
/// Derived from first principles to keep sign conventions explicit:
/// surface normal ∝ `(-dzdx, -dzdy, 1)`, sun direction
/// `(sin A·cos h, cos A·cos h, sin h)`; shade = max(0, normal·sun).
/// This is algebraically identical to the classic ESRI/Horn hillshade.
pub(crate) fn lambert_shade(
    dzdx_east: f64,
    dzdy_north: f64,
    azimuth_deg: f64,
    altitude_deg: f64,
) -> f64 {
    let az = azimuth_deg.to_radians();
    let alt = altitude_deg.to_radians();
    let (sun_x, sun_y, sun_z) = (az.sin() * alt.cos(), az.cos() * alt.cos(), alt.sin());

    let norm = (1.0 + dzdx_east * dzdx_east + dzdy_north * dzdy_north).sqrt();
    let dot = (-dzdx_east * sun_x - dzdy_north * sun_y + sun_z) / norm;
    dot.max(0.0)
}

/// Full per-pixel pipeline: Horn gradient → shade → slope darkening →
/// 8-bit gray. Flat terrain maps to `sin(45°)·255 ≈ 180`.
pub(crate) fn shade_pixel(window: &[[f64; 3]; 3], dx_m: f64, dy_m: f64) -> u8 {
    let (dzdx, dzdy) = horn_gradient(window, dx_m, dy_m);
    let shade = lambert_shade(dzdx, dzdy, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);

    let slope = (dzdx * dzdx + dzdy * dzdy).sqrt().atan();
    let darken = 1.0 - SLOPE_DARKEN_STRENGTH * (slope / SLOPE_DARKEN_FULL_AT_RAD).min(1.0);

    (shade * darken * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3×3 window of `f(x_east, y_north)` sampled at `spacing` meters,
    /// centered on `(cx, cy)`.
    fn window_of(f: impl Fn(f64, f64) -> f64, cx: f64, cy: f64, spacing: f64) -> [[f64; 3]; 3] {
        let mut w = [[0.0; 3]; 3];
        for (r, row) in w.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let x = cx + (c as f64 - 1.0) * spacing;
                let y = cy + (1.0 - r as f64) * spacing; // row 0 = north
                *cell = f(x, y);
            }
        }
        w
    }

    #[test]
    fn horn_gradient_exact_on_plane() {
        let (a, b) = (0.13, -0.07);
        let w = window_of(|x, y| 5.0 + a * x + b * y, 100.0, -40.0, 25.0);
        let (dzdx, dzdy) = horn_gradient(&w, 25.0, 25.0);
        assert!((dzdx - a).abs() < 1e-12);
        assert!((dzdy - b).abs() < 1e-12);
    }

    #[test]
    fn flat_terrain_shade_is_sun_altitude_sine() {
        let shade = lambert_shade(0.0, 0.0, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
        assert!((shade - 45f64.to_radians().sin()).abs() < 1e-12);
        let flat = window_of(|_, _| 312.0, 0.0, 0.0, 30.0);
        assert_eq!(shade_pixel(&flat, 30.0, 30.0), 180);
    }

    #[test]
    fn slope_facing_sun_brightens_and_opposite_darkens() {
        let t = 0.3;
        // Rising east, falling north => normal tilts north-west, toward the sun.
        let facing_nw = lambert_shade(t, -t, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
        let facing_se = lambert_shade(-t, t, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
        let flat = lambert_shade(0.0, 0.0, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
        assert!(facing_nw > flat + 0.05, "nw {facing_nw} flat {flat}");
        assert!(facing_se < flat - 0.05, "se {facing_se} flat {flat}");
    }

    #[test]
    fn sun_facing_aspect_is_brightest_of_compass_planes() {
        let t = 0.4;
        // (dzdx, dzdy) such that the normal faces each compass direction.
        let aspects: Vec<(&str, f64, f64)> = vec![
            ("N", 0.0, -t),
            ("NE", -t, -t),
            ("E", -t, 0.0),
            ("SE", -t, t),
            ("S", 0.0, t),
            ("SW", t, t),
            ("W", t, 0.0),
            ("NW", t, -t),
        ];
        let brightest = aspects
            .iter()
            .max_by(|a, b| {
                let sa = lambert_shade(a.1, a.2, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
                let sb = lambert_shade(b.1, b.2, SUN_AZIMUTH_DEG, SUN_ALTITUDE_DEG);
                sa.total_cmp(&sb)
            })
            .map(|a| a.0)
            .unwrap_or("");
        assert_eq!(brightest, "NW");
    }

    #[test]
    fn cone_flanks_shade_by_aspect() {
        // Downward cone z = 1000 − 0.5·r around the origin: the NW flank
        // faces the sun, the SE flank faces away.
        let cone = |x: f64, y: f64| 1000.0 - 0.5 * (x * x + y * y).sqrt();
        let spacing = 30.0;
        let nw = shade_pixel(&window_of(cone, -300.0, 300.0, spacing), spacing, spacing);
        let se = shade_pixel(&window_of(cone, 300.0, -300.0, spacing), spacing, spacing);
        let flat = shade_pixel(&window_of(|_, _| 0.0, 0.0, 0.0, spacing), spacing, spacing);
        assert!(nw > flat, "NW flank {nw} should beat flat {flat}");
        assert!(se < flat, "SE flank {se} should be darker than flat {flat}");
    }

    #[test]
    fn slope_darkening_dims_steep_sun_facing_slopes() {
        // A steep sun-facing slope shades close to 1.0 before darkening;
        // with the slope penalty it must come out clearly below 255.
        let t = 45f64.to_radians().tan();
        let w = window_of(|x, y| t * x - t * y, 0.0, 0.0, 1.0);
        let px = shade_pixel(&w, 1.0, 1.0);
        assert!(px < 250, "expected slope darkening, got {px}");
        assert!(px > 150);
    }
}

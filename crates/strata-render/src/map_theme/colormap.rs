//! Piecewise-linear colormaps for the gridded weather overlays.
//!
//! Design decision: the colormap is a **piecewise function evaluated in the
//! shader** (stops uploaded in the per-field uniform), not a 1D LUT
//! texture. With three fields of at most [`MAX_COLORMAP_STOPS`] stops each
//! there is nothing to gain from a LUT, and the uniform path means a theme
//! switch is just the next uniform write — no texture regeneration, no
//! extra bindings. [`Colormap::sample`] is the pure CPU mirror of the
//! shader evaluation (`colormap_sample` in `shaders/weather_grid.wgsl`)
//! and what the breakpoint tests exercise.
//!
//! Colors are **premultiplied linear** RGBA like the rest of
//! [`crate::map_theme::WeatherTheme`]; linear interpolation of
//! premultiplied colors is itself premultiplied, so blending stays
//! consistent end to end.

/// Maximum number of stops a [`Colormap`] can carry — mirrored by the
/// uniform layout in `shaders/weather_grid.wgsl`.
pub const MAX_COLORMAP_STOPS: usize = 8;

/// One colormap stop: a field value and its premultiplied linear RGBA.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    /// Field value in the field's documented unit
    /// (see [`crate::features::GriddedField`]).
    pub value: f32,
    /// Premultiplied linear RGBA at this value.
    pub color: [f32; 4],
}

/// A piecewise-linear colormap over a scalar field.
///
/// Values **below the first stop clamp to the first stop's color** (themes
/// author that stop fully transparent to get a threshold), values above the
/// last stop clamp to the last stop's color. Stops must be strictly
/// ascending in value; at most [`MAX_COLORMAP_STOPS`] are kept (extras are
/// dropped with a debug assertion — the theme completeness tests pin the
/// built-in maps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colormap {
    stops: [ColorStop; MAX_COLORMAP_STOPS],
    len: usize,
}

const TRANSPARENT: ColorStop = ColorStop {
    value: 0.0,
    color: [0.0; 4],
};

impl Colormap {
    /// Build a colormap from ascending stops.
    pub fn new(stops: &[ColorStop]) -> Self {
        debug_assert!(
            stops.len() <= MAX_COLORMAP_STOPS,
            "colormap has more than {MAX_COLORMAP_STOPS} stops"
        );
        debug_assert!(
            stops.windows(2).all(|w| w[0].value < w[1].value),
            "colormap stops must be strictly ascending"
        );
        let len = stops.len().min(MAX_COLORMAP_STOPS);
        let mut padded = [TRANSPARENT; MAX_COLORMAP_STOPS];
        padded[..len].copy_from_slice(&stops[..len]);
        Self { stops: padded, len }
    }

    /// The stops in ascending order.
    pub fn stops(&self) -> &[ColorStop] {
        &self.stops[..self.len]
    }

    /// Evaluate the colormap — the pure CPU mirror of `colormap_sample` in
    /// `shaders/weather_grid.wgsl`. Non-finite values (no data) and an
    /// empty map yield fully transparent.
    pub fn sample(&self, value: f32) -> [f32; 4] {
        if !value.is_finite() {
            return [0.0; 4];
        }
        let stops = self.stops();
        let Some(first) = stops.first() else {
            return [0.0; 4];
        };
        if value <= first.value {
            return first.color;
        }
        for pair in stops.windows(2) {
            let (lo, hi) = (pair[0], pair[1]);
            if value <= hi.value {
                let t = (value - lo.value) / (hi.value - lo.value);
                return lerp(lo.color, hi.color, t);
            }
        }
        // `stops` is non-empty here.
        stops[stops.len() - 1].color
    }

    /// Largest stop alpha — the overlay's peak opacity.
    pub fn max_alpha(&self) -> f32 {
        self.stops().iter().map(|s| s.color[3]).fold(0.0, f32::max)
    }
}

fn lerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Colormap {
        Colormap::new(&[
            ColorStop {
                value: 10.0,
                color: [0.0; 4],
            },
            ColorStop {
                value: 20.0,
                color: [0.4, 0.2, 0.0, 0.4],
            },
            ColorStop {
                value: 40.0,
                color: [0.8, 0.8, 0.0, 0.8],
            },
        ])
    }

    #[test]
    fn clamps_below_first_and_above_last_stop() {
        let m = map();
        assert_eq!(m.sample(-5.0), [0.0; 4], "below first stop");
        assert_eq!(m.sample(10.0), [0.0; 4], "exact first stop");
        assert_eq!(m.sample(40.0), [0.8, 0.8, 0.0, 0.8], "exact last stop");
        assert_eq!(m.sample(1e9), [0.8, 0.8, 0.0, 0.8], "above last stop");
    }

    #[test]
    fn interpolates_linearly_between_stops() {
        let m = map();
        assert_eq!(m.sample(15.0), [0.2, 0.1, 0.0, 0.2], "midpoint segment 0");
        assert_eq!(m.sample(20.0), [0.4, 0.2, 0.0, 0.4], "exact interior stop");
        assert_eq!(m.sample(30.0), [0.6, 0.5, 0.0, 0.6], "midpoint segment 1");
    }

    #[test]
    fn non_finite_and_empty_are_transparent() {
        assert_eq!(map().sample(f32::NAN), [0.0; 4]);
        assert_eq!(map().sample(f32::INFINITY), [0.0; 4]);
        assert_eq!(map().sample(f32::NEG_INFINITY), [0.0; 4]);
        let empty = Colormap::new(&[]);
        assert_eq!(empty.sample(1.0), [0.0; 4]);
        assert_eq!(empty.max_alpha(), 0.0);
    }

    #[test]
    fn max_alpha_is_the_peak_stop_alpha() {
        assert_eq!(map().max_alpha(), 0.8);
    }
}

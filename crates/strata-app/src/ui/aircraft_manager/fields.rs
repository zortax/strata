//! Numeric field plumbing shared by the editor sections: parse (tolerant
//! of comma decimal separators), clamp ranges and canonical display
//! formatting. Pure helpers — the `InputState` binding lives in
//! [`super::editor`].

/// Clamp range + display precision of one numeric profile field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NumOpts {
    pub min: f64,
    pub max: f64,
    /// Maximum fraction digits shown (trailing zeros trimmed).
    pub decimals: usize,
    /// Optional fields treat an empty input as `None` ("not published");
    /// required fields keep their previous value until a parseable edit.
    pub optional: bool,
}

impl NumOpts {
    pub const fn new(min: f64, max: f64, decimals: usize) -> Self {
        Self {
            min,
            max,
            decimals,
            optional: false,
        }
    }

    /// Non-negative quantity (mass, volume, speed, distance, rate).
    pub const fn positive(max: f64, decimals: usize) -> Self {
        Self::new(0.0, max, decimals)
    }

    /// Marks the field optional (empty input = `None`).
    pub const fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    pub fn clamp(&self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }
}

/// Parses a user-typed number; `,` works as the decimal separator (German
/// keyboards) and surrounding whitespace is ignored. `None` for anything
/// unparsable **or non-finite** — callers leave the previous value alone.
pub(super) fn parse_num(text: &str) -> Option<f64> {
    let normalized = text.trim().replace(',', ".");
    match normalized.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => None,
    }
}

/// Canonical display of a field value: fixed precision with trailing zeros
/// (and a dangling separator) trimmed — `1.20` → `"1.2"`, `3.00` → `"3"`.
pub(super) fn format_num(value: f64, decimals: usize) -> String {
    let mut text = format!("{value:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    // Avoid the IEEE negative zero artifact.
    if text == "-0" { "0".to_owned() } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_comma_and_rejects_junk() {
        assert_eq!(parse_num(" 36.5 "), Some(36.5));
        assert_eq!(parse_num("36,5"), Some(36.5));
        assert_eq!(parse_num("-0,1"), Some(-0.1));
        assert_eq!(parse_num(""), None);
        assert_eq!(parse_num("12a"), None);
        assert_eq!(parse_num("NaN"), None);
        assert_eq!(parse_num("inf"), None);
    }

    #[test]
    fn format_trims_trailing_zeros() {
        assert_eq!(format_num(1.20, 2), "1.2");
        assert_eq!(format_num(3.00, 2), "3");
        assert_eq!(format_num(0.72, 2), "0.72");
        assert_eq!(format_num(1157.0, 0), "1157");
        assert_eq!(format_num(-0.0, 1), "0");
        assert_eq!(format_num(0.349, 2), "0.35");
    }

    #[test]
    fn clamping_respects_the_field_range() {
        let opts = NumOpts::positive(9999.0, 1);
        assert_eq!(opts.clamp(-3.0), 0.0);
        assert_eq!(opts.clamp(10_500.0), 9999.0);
        assert_eq!(opts.clamp(42.0), 42.0);

        // Correction factors are allowed to be negative (headwind −0.10).
        let factors = NumOpts::new(-1.0, 5.0, 2);
        assert_eq!(factors.clamp(-0.10), -0.10);
        assert_eq!(factors.clamp(-2.0), -1.0);
    }
}

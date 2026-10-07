//! The Echo Music colour system.
//!
//! Upstream renders a Material You palette generated from a seed colour. On
//! desktop we reproduce that behaviour: the seed `#ED5564` is converted into a
//! full tonal scheme (dark and light) using an HSL approximation of the
//! Material 3 tonal-spot algorithm. Any user-chosen accent flows through the
//! same generator, so custom theme colours behave identically to the app.

use serde::{Deserialize, Serialize};

/// The signature Echo Music accent (`#ED5564`), identical to upstream.
pub const ECHO_SEED: Rgb = Rgb::new(0xED, 0x55, 0x64);

/// An opaque 8-bit RGB colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self(r, g, b)
    }

    /// Parses `"#RRGGBB"`, `"RRGGBB"` or `"#RGB"`.
    pub fn parse(input: &str) -> Option<Self> {
        let hex = input.trim().trim_start_matches('#');
        match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                Some(Self::new(r, g, b))
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::new(r, g, b))
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }

    /// `#RRGGBBAA` — used for the translucent surfaces the design leans on.
    pub fn to_hex_alpha(self, alpha: u8) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.0, self.1, self.2, alpha)
    }

    fn to_hsl(self) -> (f32, f32, f32) {
        let r = self.0 as f32 / 255.0;
        let g = self.1 as f32 / 255.0;
        let b = self.2 as f32 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        if (max - min).abs() < f32::EPSILON {
            return (0.0, 0.0, l);
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        (h * 360.0, s, l)
    }

    fn from_hsl(h: f32, s: f32, l: f32) -> Self {
        let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
        let s = s.clamp(0.0, 1.0);
        let l = l.clamp(0.0, 1.0);
        if s <= f32::EPSILON {
            let v = (l * 255.0).round() as u8;
            return Self::new(v, v, v);
        }
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        let channel = |mut t: f32| {
            if t < 0.0 {
                t += 1.0;
            }
            if t > 1.0 {
                t -= 1.0;
            }
            let value = if t < 1.0 / 6.0 {
                p + (q - p) * 6.0 * t
            } else if t < 1.0 / 2.0 {
                q
            } else if t < 2.0 / 3.0 {
                p + (q - p) * (2.0 / 3.0 - t) * 6.0
            } else {
                p
            };
            (value * 255.0).round().clamp(0.0, 255.0) as u8
        };
        Self::new(channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
    }

    /// Relative luminance per WCAG, used to decide on-colour contrast.
    pub fn luminance(self) -> f32 {
        let f = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(self.0) + 0.7152 * f(self.1) + 0.0722 * f(self.2)
    }

    /// Picks black or white text for maximum contrast against this colour.
    pub fn contrasting_text(self) -> Self {
        if self.luminance() > 0.45 {
            Self::new(0x11, 0x11, 0x11)
        } else {
            Self::new(0xFF, 0xFF, 0xFF)
        }
    }
}

/// A complete Material 3 style colour scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Palette {
    pub primary: Rgb,
    pub on_primary: Rgb,
    pub primary_container: Rgb,
    pub on_primary_container: Rgb,
    pub secondary: Rgb,
    pub on_secondary: Rgb,
    pub secondary_container: Rgb,
    pub on_secondary_container: Rgb,
    pub tertiary: Rgb,
    pub on_tertiary: Rgb,
    pub tertiary_container: Rgb,
    pub on_tertiary_container: Rgb,
    pub error: Rgb,
    pub on_error: Rgb,
    pub background: Rgb,
    pub on_background: Rgb,
    pub surface: Rgb,
    pub on_surface: Rgb,
    pub surface_variant: Rgb,
    pub on_surface_variant: Rgb,
    pub surface_container_lowest: Rgb,
    pub surface_container_low: Rgb,
    pub surface_container: Rgb,
    pub surface_container_high: Rgb,
    pub surface_container_highest: Rgb,
    pub outline: Rgb,
    pub outline_variant: Rgb,
    pub inverse_surface: Rgb,
    pub inverse_on_surface: Rgb,
    pub inverse_primary: Rgb,
    pub scrim: Rgb,
    pub is_dark: bool,
}

impl Palette {
    /// Generates the dark scheme for a given accent seed.
    pub fn dark(seed: Rgb) -> Self {
        let (h, s, _) = seed.to_hsl();
        let s = s.max(0.35);
        let tint = |l: f32, sat: f32| Rgb::from_hsl(h, sat, l);
        let neutral = |l: f32| Rgb::from_hsl(h, 0.10, l);
        Self {
            primary: tint(0.80, s * 0.72),
            on_primary: tint(0.20, s * 0.90),
            primary_container: tint(0.30, s * 0.70),
            on_primary_container: tint(0.92, s * 0.85),
            secondary: tint(0.78, s * 0.22),
            on_secondary: tint(0.22, s * 0.30),
            secondary_container: tint(0.30, s * 0.20),
            on_secondary_container: tint(0.90, s * 0.30),
            tertiary: Rgb::from_hsl(h + 55.0, s * 0.38, 0.78),
            on_tertiary: Rgb::from_hsl(h + 55.0, s * 0.45, 0.22),
            tertiary_container: Rgb::from_hsl(h + 55.0, s * 0.35, 0.30),
            on_tertiary_container: Rgb::from_hsl(h + 55.0, s * 0.40, 0.90),
            error: Rgb::new(0xFF, 0xB4, 0xAB),
            on_error: Rgb::new(0x69, 0x00, 0x05),
            background: neutral(0.07),
            on_background: neutral(0.91),
            surface: neutral(0.07),
            on_surface: neutral(0.91),
            surface_variant: tint(0.30, s * 0.14),
            on_surface_variant: tint(0.80, s * 0.12),
            surface_container_lowest: neutral(0.05),
            surface_container_low: neutral(0.10),
            surface_container: neutral(0.12),
            surface_container_high: neutral(0.15),
            surface_container_highest: neutral(0.19),
            outline: neutral(0.60),
            outline_variant: neutral(0.31),
            inverse_surface: neutral(0.91),
            inverse_on_surface: neutral(0.19),
            inverse_primary: tint(0.42, s * 0.85),
            scrim: Rgb::new(0, 0, 0),
            is_dark: true,
        }
    }

    /// Generates the light scheme for a given accent seed.
    pub fn light(seed: Rgb) -> Self {
        let (h, s, _) = seed.to_hsl();
        let s = s.max(0.35);
        let tint = |l: f32, sat: f32| Rgb::from_hsl(h, sat, l);
        let neutral = |l: f32| Rgb::from_hsl(h, 0.09, l);
        Self {
            primary: tint(0.42, s * 0.85),
            on_primary: Rgb::new(0xFF, 0xFF, 0xFF),
            primary_container: tint(0.90, s * 0.80),
            on_primary_container: tint(0.13, s * 0.90),
            secondary: tint(0.42, s * 0.30),
            on_secondary: Rgb::new(0xFF, 0xFF, 0xFF),
            secondary_container: tint(0.90, s * 0.25),
            on_secondary_container: tint(0.16, s * 0.35),
            tertiary: Rgb::from_hsl(h + 55.0, s * 0.40, 0.40),
            on_tertiary: Rgb::new(0xFF, 0xFF, 0xFF),
            tertiary_container: Rgb::from_hsl(h + 55.0, s * 0.45, 0.90),
            on_tertiary_container: Rgb::from_hsl(h + 55.0, s * 0.50, 0.15),
            error: Rgb::new(0xBA, 0x1A, 0x1A),
            on_error: Rgb::new(0xFF, 0xFF, 0xFF),
            background: neutral(0.99),
            on_background: neutral(0.12),
            surface: neutral(0.99),
            on_surface: neutral(0.12),
            surface_variant: tint(0.90, s * 0.22),
            on_surface_variant: tint(0.30, s * 0.14),
            surface_container_lowest: Rgb::new(0xFF, 0xFF, 0xFF),
            surface_container_low: neutral(0.96),
            surface_container: neutral(0.94),
            surface_container_high: neutral(0.92),
            surface_container_highest: neutral(0.90),
            outline: neutral(0.50),
            outline_variant: neutral(0.80),
            inverse_surface: neutral(0.20),
            inverse_on_surface: neutral(0.95),
            inverse_primary: tint(0.80, s * 0.72),
            scrim: Rgb::new(0, 0, 0),
            is_dark: false,
        }
    }

    /// Convenience: pick dark/light from a boolean.
    pub fn from_seed(seed: Rgb, dark: bool) -> Self {
        if dark {
            Self::dark(seed)
        } else {
            Self::light(seed)
        }
    }

    /// Pure-black variant for OLED panels, matching upstream's `pureBlack`.
    pub fn pure_black(mut self) -> Self {
        if self.is_dark {
            self.surface = Rgb::new(0, 0, 0);
            self.background = Rgb::new(0, 0, 0);
            self.surface_container_lowest = Rgb::new(0, 0, 0);
            self.surface_container_low = Rgb::new(0x0A, 0x0A, 0x0A);
        }
        self
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark(ECHO_SEED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_variants() {
        assert_eq!(Rgb::parse("#ED5564"), Some(Rgb::new(0xED, 0x55, 0x64)));
        assert_eq!(Rgb::parse("ED5564"), Some(Rgb::new(0xED, 0x55, 0x64)));
        assert_eq!(Rgb::parse("#FFF"), Some(Rgb::new(255, 255, 255)));
        assert_eq!(Rgb::parse("nope"), None);
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(ECHO_SEED.to_hex(), "#ED5564");
        assert_eq!(ECHO_SEED.to_hex_alpha(0x4D), "#ED55644D");
    }

    #[test]
    fn dark_palette_is_dark_and_light_is_light() {
        let dark = Palette::dark(ECHO_SEED);
        let light = Palette::light(ECHO_SEED);
        assert!(dark.surface.luminance() < 0.1);
        assert!(light.surface.luminance() > 0.8);
        assert!(dark.on_surface.luminance() > dark.surface.luminance());
    }

    #[test]
    fn contrasting_text_flips() {
        assert_eq!(
            Rgb::new(255, 255, 255).contrasting_text(),
            Rgb::new(0x11, 0x11, 0x11)
        );
        assert_eq!(
            Rgb::new(0, 0, 0).contrasting_text(),
            Rgb::new(0xFF, 0xFF, 0xFF)
        );
    }
}

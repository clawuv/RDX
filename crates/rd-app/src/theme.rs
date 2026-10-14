//! Adapted from SquirrelDisk / DCX (AGPL-3.0-only).
#![allow(dead_code)]
use eframe::egui::Color32;

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub dark: bool,
    /// Window background (top / bottom of a vertical gradient).
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    /// Cards, list hover, inputs.
    pub surface: Color32,
    pub surface_hi: Color32,
    pub stroke: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent2: Color32,
    pub danger: Color32,
    pub ok: Color32,
    pub warn: Color32,
    /// Cyclic gradient used to colour the sunburst by angle.
    pub wheel: &'static [[u8; 3]],
    /// Colour of aggregates ("small files", hidden space).
    pub neutral: Color32,
    /// How much deeper rings get lighter (can be negative on light themes).
    pub depth_light: f32,
}

const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn all() -> Vec<Theme> {
    vec![
        Theme {
            name: "Hazelnut",
            dark: true,
            bg_top: c(0x2a, 0x1c, 0x3c),
            bg_bottom: c(0x1a, 0x12, 0x27),
            surface: c(0x36, 0x27, 0x4c),
            surface_hi: c(0x45, 0x33, 0x60),
            stroke: c(0x4d, 0x3b, 0x68),
            text: c(0xf4, 0xef, 0xfa),
            text_dim: c(0xb9, 0xac, 0xcc),
            text_faint: c(0x7d, 0x6f, 0x93),
            accent: c(0x8e, 0x6b, 0xff),
            accent2: c(0xff, 0x7e, 0xc8),
            danger: c(0xe8, 0x3a, 0x4f),
            ok: c(0xa6, 0xe2, 0x2e),
            warn: c(0xff, 0xb3, 0x47),
            wheel: &[
                [0x39, 0x6b, 0xff],
                [0x8b, 0x4d, 0xff],
                [0xd9, 0x46, 0xef],
                [0xff, 0x4f, 0x9a],
                [0xff, 0x70, 0x5a],
                [0xff, 0xb3, 0x47],
                [0xff, 0xe4, 0x5c],
                [0xb8, 0xf0, 0x4a],
                [0x4c, 0xe0, 0x6a],
                [0x2f, 0xd8, 0xc0],
                [0x3a, 0xc8, 0xff],
            ],
            neutral: c(0x6e, 0x60, 0x82),
            depth_light: 0.03,
        },
        Theme {
            name: "Midnight Neon",
            dark: true,
            bg_top: c(0x0d, 0x12, 0x24),
            bg_bottom: c(0x05, 0x07, 0x10),
            surface: c(0x16, 0x1e, 0x36),
            surface_hi: c(0x20, 0x2b, 0x4a),
            stroke: c(0x27, 0x34, 0x58),
            text: c(0xe8, 0xf1, 0xff),
            text_dim: c(0x9a, 0xab, 0xcc),
            text_faint: c(0x5a, 0x69, 0x8c),
            accent: c(0x00, 0xe5, 0xff),
            accent2: c(0xff, 0x2e, 0xd1),
            danger: c(0xff, 0x3b, 0x6b),
            ok: c(0x39, 0xff, 0x9c),
            warn: c(0xff, 0xc4, 0x00),
            wheel: &[
                [0x00, 0xe5, 0xff],
                [0x2f, 0x7b, 0xff],
                [0x7a, 0x3c, 0xff],
                [0xc9, 0x2e, 0xff],
                [0xff, 0x2e, 0xd1],
                [0xff, 0x3b, 0x6b],
                [0xff, 0x8a, 0x00],
                [0xff, 0xe1, 0x00],
                [0x39, 0xff, 0x9c],
            ],
            neutral: c(0x4a, 0x58, 0x7a),
            depth_light: 0.03,
        },
        Theme {
            name: "Sunset",
            dark: true,
            bg_top: c(0x3a, 0x1a, 0x2e),
            bg_bottom: c(0x1c, 0x0c, 0x1c),
            surface: c(0x4a, 0x24, 0x3b),
            surface_hi: c(0x5c, 0x2e, 0x49),
            stroke: c(0x63, 0x34, 0x50),
            text: c(0xff, 0xf1, 0xe8),
            text_dim: c(0xe0, 0xb8, 0xb4),
            text_faint: c(0x9c, 0x70, 0x80),
            accent: c(0xff, 0x8c, 0x5a),
            accent2: c(0xff, 0xd1, 0x66),
            danger: c(0xff, 0x3d, 0x5e),
            ok: c(0xc6, 0xe3, 0x77),
            warn: c(0xff, 0xd1, 0x66),
            wheel: &[
                [0x5b, 0x2a, 0x86],
                [0x9a, 0x2f, 0x8e],
                [0xd9, 0x3b, 0x7a],
                [0xff, 0x5a, 0x5f],
                [0xff, 0x8c, 0x5a],
                [0xff, 0xb8, 0x5c],
                [0xff, 0xe0, 0x7a],
                [0xff, 0xa6, 0x9e],
            ],
            neutral: c(0x7a, 0x55, 0x6a),
            depth_light: 0.03,
        },
        Theme {
            name: "Paper",
            dark: false,
            bg_top: c(0xfb, 0xf8, 0xf3),
            bg_bottom: c(0xee, 0xe9, 0xe0),
            surface: c(0xff, 0xff, 0xff),
            surface_hi: c(0xf1, 0xec, 0xe4),
            stroke: c(0xdd, 0xd5, 0xc8),
            text: c(0x2b, 0x25, 0x1f),
            text_dim: c(0x6b, 0x61, 0x57),
            text_faint: c(0xa3, 0x98, 0x8b),
            accent: c(0xd9, 0x6c, 0x2c),
            accent2: c(0x3f, 0x7c, 0xac),
            danger: c(0xd6, 0x33, 0x3f),
            ok: c(0x4f, 0x9d, 0x4a),
            warn: c(0xd9, 0x96, 0x1c),
            wheel: &[
                [0x3f, 0x7c, 0xac],
                [0x6a, 0x5a, 0xcd],
                [0xb0, 0x4f, 0xb6],
                [0xe0, 0x5a, 0x7a],
                [0xe8, 0x7a, 0x3c],
                [0xe8, 0xb4, 0x2c],
                [0x9c, 0xc0, 0x3a],
                [0x3f, 0xad, 0x6a],
                [0x2a, 0xa8, 0xa8],
            ],
            neutral: c(0xb8, 0xae, 0xa0),
            depth_light: -0.02,
        },
    ]
}

/// Cool neutral palette adapted to the supplied settings-page reference.
pub fn mist() -> Theme {
    let mut theme = by_name("Paper");
    theme.name = "Mist";
    theme.bg_top = c(0xfa, 0xfa, 0xfb);
    theme.bg_bottom = c(0xe2, 0xe3, 0xe5);
    theme.surface = c(0xff, 0xff, 0xff);
    theme.surface_hi = c(0xed, 0xee, 0xf0);
    theme.stroke = c(0xdd, 0xde, 0xe1);
    theme.text = c(0x1c, 0x1d, 0x20);
    theme.text_dim = c(0x6d, 0x6e, 0x73);
    theme.text_faint = c(0x94, 0x95, 0x9b);
    // A deep, clean blue for toggles / primary actions; the old warm gray
    // read as a muddy blob on the paper-white surfaces.
    theme.accent = c(0x25, 0x63, 0xeb);
    theme.accent2 = c(0x3b, 0x82, 0xf6);
    theme.ok = c(0x58, 0xa4, 0x80);
    theme.warn = c(0xd8, 0xa4, 0x44);
    theme.neutral = c(0xb9, 0xba, 0xbf);
    theme
}

/// Charcoal counterpart to Mist, matching the reference's lighter sidebar.
pub fn mist_dark() -> Theme {
    let mut t = mist();
    t.name = "Mist Dark";
    t.dark = true;
    t.bg_top = c(39, 41, 41);
    t.bg_bottom = c(30, 32, 32);
    t.surface = c(39, 41, 41);
    t.surface_hi = c(49, 51, 51);
    t.stroke = c(59, 61, 61);
    t.text = c(235, 237, 237);
    t.text_dim = c(180, 184, 184);
    t.text_faint = c(145, 150, 150);
    // Neutral green-gray accents look muddy on charcoal; a clear blue reads
    // far better for toggles, primary buttons and selection states.
    t.accent = c(59, 130, 246);
    t.accent2 = c(96, 165, 250);
    t.neutral = c(82, 87, 87);
    t
}

pub fn by_name(name: &str) -> Theme {
    let v = all();
    v.iter()
        .find(|t| t.name == name)
        .cloned()
        .unwrap_or_else(|| v[0].clone())
}

// ---------------------------------------------------------------------------
// colour helpers (HSL in linear-ish sRGB space, good enough for UI work)

pub fn to_hsl(c: Color32) -> (f32, f32, f32) {
    let r = c.r() as f32 / 255.0;
    let g = c.g() as f32 / 255.0;
    let b = c.b() as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

pub fn from_hsl(h: f32, s: f32, l: f32) -> Color32 {
    let hue = |p: f32, q: f32, mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let (s, l) = (s.clamp(0.0, 1.0), l.clamp(0.0, 1.0));
    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return Color32::from_rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue(p, q, h + 1.0 / 3.0);
    let g = hue(p, q, h);
    let b = hue(p, q, h - 1.0 / 3.0);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        c.r(),
        c.g(),
        c.b(),
        (a.clamp(0.0, 1.0) * c.a() as f32) as u8,
    )
}

pub fn lighten(c: Color32, amount: f32) -> Color32 {
    let (h, s, l) = to_hsl(c);
    from_hsl(h, s, l + amount)
}

impl Theme {
    /// Colour of the wheel gradient at `t` in [0,1) (cyclic).
    pub fn wheel_at(&self, t: f32) -> Color32 {
        let n = self.wheel.len();
        let x = t.rem_euclid(1.0) * n as f32;
        let i = x.floor() as usize % n;
        let j = (i + 1) % n;
        let f = x.fract();
        let a = self.wheel[i];
        let b = self.wheel[j];
        lerp_color(c(a[0], a[1], a[2]), c(b[0], b[1], b[2]), f)
    }

    /// The big rounded sheet the screens live on (Motrix-style inset):
    /// halfway between the window background and the card colour, so cards
    /// drawn on it still stand out.
    pub fn inset_bg(&self) -> Color32 {
        if self.name == "Mist" {
            return c(0xf7, 0xf7, 0xf8);
        }
        if self.name == "Mist Dark" {
            return self.bg_bottom;
        }
        let bg = lerp_color(self.bg_top, self.bg_bottom, 0.5);
        lerp_color(bg, self.surface, if self.dark { 0.45 } else { 0.6 })
    }

    /// Flat window backdrop (sidebar + margins around the sheet). A step
    /// darker than [`Theme::inset_bg`] except on Mist Dark, whose reference
    /// uses a lighter charcoal sidebar around a darker content area.
    pub fn shell_bg(&self) -> Color32 {
        if self.name == "Mist" {
            return self.bg_bottom;
        }
        if self.name == "Mist Dark" {
            return self.bg_top;
        }
        if self.dark {
            lerp_color(self.bg_bottom, Color32::BLACK, 0.12)
        } else {
            lerp_color(self.bg_bottom, self.stroke, 0.3)
        }
    }

    /// Aggregates ("smaller items", "smaller files"): a shade just darker than the background.
    pub fn aggregate_color(&self) -> Color32 {
        let bg = lerp_color(self.bg_top, self.bg_bottom, 0.5);
        lerp_color(bg, Color32::BLACK, if self.dark { 0.14 } else { 0.07 })
    }

    /// Final colour of a segment given its angular midpoint and depth.
    pub fn segment_color(&self, mid: f32, depth: usize, is_file: bool) -> Color32 {
        let base = self.wheel_at(mid);
        let (h, s, l) = to_hsl(base);
        let d = depth.saturating_sub(1) as f32;
        let mut l2 = l + d * self.depth_light;
        let mut s2 = s * (1.0 - d * 0.025);
        if is_file {
            l2 += 0.04 * self.depth_light.signum();
            s2 *= 0.85;
        }
        from_hsl(h, s2, l2.clamp(0.12, 0.88))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charcoal_backgrounds_match_reference_hierarchy() {
        let theme = mist_dark();
        assert!(theme.dark);
        assert_eq!(theme.shell_bg(), c(39, 41, 41));
        assert_eq!(theme.inset_bg(), c(30, 32, 32));
        assert!(theme.surface_hi.r() > theme.surface.r());
        assert!(theme.text.r() > theme.text_dim.r());
        assert!(theme.text_dim.r() > theme.text_faint.r());
    }

    #[test]
    fn light_backgrounds_remain_unchanged() {
        let theme = mist();
        assert!(!theme.dark);
        assert_eq!(theme.shell_bg(), c(0xe2, 0xe3, 0xe5));
        assert_eq!(theme.inset_bg(), c(0xf7, 0xf7, 0xf8));
    }
}

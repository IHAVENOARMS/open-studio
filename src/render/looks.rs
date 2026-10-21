//! Camera colour grading: twelve built-in looks and .cube LUT files. The
//! same maths runs in the shader (fs_camera) and on the CPU for the look
//! thumbnails in the Camera Style tab.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Look {
    #[default]
    None,
    Natural,
    Warm,
    Cool,
    Vivid,
    Soft,
    Film,
    Golden,
    TealOrange,
    Rose,
    Faded,
    Mono,
    Noir,
}

/// Grade parameters: exposure (stops), contrast, saturation, temperature,
/// tint, fade (lifted blacks), split toning amount, mono.
#[derive(Clone, Copy, Debug, Default)]
pub struct Grade {
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
    pub fade: f32,
    pub split: f32,
    pub mono: f32,
}

impl Look {
    pub const ALL: [Look; 12] = [
        Look::Natural,
        Look::Warm,
        Look::Cool,
        Look::Vivid,
        Look::Soft,
        Look::Film,
        Look::Golden,
        Look::TealOrange,
        Look::Rose,
        Look::Faded,
        Look::Mono,
        Look::Noir,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Look::None => "None",
            Look::Natural => "Natural",
            Look::Warm => "Warm",
            Look::Cool => "Cool",
            Look::Vivid => "Vivid",
            Look::Soft => "Soft",
            Look::Film => "Film",
            Look::Golden => "Golden",
            Look::TealOrange => "Teal & Orange",
            Look::Rose => "Rose",
            Look::Faded => "Faded",
            Look::Mono => "Mono",
            Look::Noir => "Noir",
        }
    }

    pub fn grade(self) -> Grade {
        let g = |exposure, contrast, saturation, temperature, tint, fade, split, mono| Grade {
            exposure,
            contrast,
            saturation,
            temperature,
            tint,
            fade,
            split,
            mono,
        };
        match self {
            Look::None => g(0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            Look::Natural => g(0.08, 1.06, 1.08, 0.02, 0.0, 0.0, 0.0, 0.0),
            Look::Warm => g(0.05, 1.05, 1.05, 0.18, 0.02, 0.0, 0.0, 0.0),
            Look::Cool => g(0.0, 1.05, 0.95, -0.16, -0.02, 0.0, 0.0, 0.0),
            Look::Vivid => g(0.05, 1.15, 1.4, 0.0, 0.0, 0.0, 0.0, 0.0),
            Look::Soft => g(0.1, 0.88, 0.9, 0.04, 0.02, 0.05, 0.0, 0.0),
            Look::Film => g(0.0, 1.1, 0.85, 0.06, 0.0, 0.07, 0.25, 0.0),
            Look::Golden => g(0.08, 1.04, 1.1, 0.3, 0.05, 0.02, 0.0, 0.0),
            Look::TealOrange => g(0.0, 1.12, 1.15, 0.05, 0.0, 0.0, 0.7, 0.0),
            Look::Rose => g(0.05, 1.0, 1.0, 0.08, 0.14, 0.03, 0.0, 0.0),
            Look::Faded => g(0.05, 0.85, 0.7, 0.03, 0.0, 0.12, 0.0, 0.0),
            Look::Mono => g(0.05, 1.1, 1.0, 0.0, 0.0, 0.02, 0.0, 1.0),
            Look::Noir => g(-0.05, 1.35, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
        }
    }
}

impl Grade {
    /// Shader uniform: [exposure, contrast, saturation, temperature],
    /// [tint, fade, split, mono].
    pub fn uniform(&self) -> [[f32; 4]; 2] {
        [[self.exposure, self.contrast, self.saturation, self.temperature], [self.tint, self.fade, self.split, self.mono]]
    }

    /// Applies the grade to a display (sRGB-encoded) colour in 0..1. Must
    /// match `grade()` in shader.wgsl.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let mut c = c.map(|v| v * 2f32.powf(self.exposure));
        c[0] *= 1.0 + self.temperature * 0.5 + self.tint * 0.15;
        c[1] *= 1.0 - self.tint * 0.3;
        c[2] *= 1.0 - self.temperature * 0.5 + self.tint * 0.15;
        c = c.map(|v| (v - 0.5) * self.contrast + 0.5);
        let l = c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722;
        c = c.map(|v| l + (v - l) * self.saturation);
        // Split toning: teal shadows, orange highlights.
        let s = self.split * 0.12;
        let w = l.clamp(0.0, 1.0);
        c[0] += s * (w - 0.5) * 2.0;
        c[1] += s * (w - 0.5) * 0.4;
        c[2] -= s * (w - 0.5) * 2.0;
        c = c.map(|v| l + (v - l) * (1.0 - self.mono));
        c.map(|v| (self.fade + v * (1.0 - self.fade)).clamp(0.0, 1.0))
    }
}

/// A 3D LUT from a .cube file, flattened to an (n*n) x n RGBA8 image: slice
/// b is the column block b, red runs across, green down.
pub struct Lut {
    pub size: u32,
    pub rgba: Vec<u8>,
}

pub fn load_cube(path: &std::path::Path) -> anyhow::Result<Lut> {
    let text = std::fs::read_to_string(path)?;
    let mut size = 0u32;
    let mut values: Vec<[f32; 3]> = vec![];
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(n) = line.strip_prefix("LUT_3D_SIZE") {
            size = n.trim().parse()?;
            continue;
        }
        if line.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
            continue; // TITLE, DOMAIN_MIN/MAX, ...
        }
        let v: Vec<f32> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if v.len() == 3 {
            values.push([v[0], v[1], v[2]]);
        }
    }
    if size < 2 || values.len() != (size * size * size) as usize {
        anyhow::bail!("not a 3D .cube LUT");
    }
    let n = size as usize;
    let mut rgba = vec![0u8; n * n * n * 4];
    // .cube order: red fastest, then green, then blue.
    for (i, v) in values.iter().enumerate() {
        let (r, g, b) = (i % n, (i / n) % n, i / (n * n));
        let (x, y) = (b * n + r, g);
        let o = (y * n * n + x) * 4;
        for k in 0..3 {
            rgba[o + k] = (v[k].clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        rgba[o + 3] = 255;
    }
    Ok(Lut { size, rgba })
}

#[cfg(test)]
mod tests {
    #[test]
    fn identity_cube_round_trips() {
        let n = 4;
        let mut s = String::from("LUT_3D_SIZE 4\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    s += &format!("{} {} {}\n", r as f32 / 3.0, g as f32 / 3.0, b as f32 / 3.0);
                }
            }
        }
        let p = std::env::temp_dir().join("open-studio-test.cube");
        std::fs::write(&p, s).unwrap();
        let lut = super::load_cube(&p).unwrap();
        assert_eq!(lut.size, 4);
        // Entry r=3,g=0,b=1 sits at x = 1*4+3, y = 0.
        let o = (7) * 4;
        assert_eq!(&lut.rgba[o..o + 3], &[255, 0, 85]);
        assert_eq!(super::super::looks::Look::None.grade().apply([0.3, 0.5, 0.7]), [0.3, 0.5, 0.7].map(|v: f32| v));
    }
}

//! Unlit export albedo. UV inputs are decoder/top-down coordinates; mapping is
//! evaluated in the renderer's +Y-up coordinates before the single row flip.
use super::{appearance::ColorTexture, RenderParams};
type Rgb = [f32; 3];
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    std::array::from_fn(|i| a[i] * (1. - t) + b[i] * t)
}
fn rgb(s: &str) -> Rgb {
    let s = s.trim_start_matches('#');
    std::array::from_fn(|i| {
        u8::from_str_radix(s.get(i * 2..i * 2 + 2).unwrap_or("00"), 16).unwrap_or(0) as f32 / 255.
    })
}
fn linear(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}
fn srgb(x: f32) -> f32 {
    if x < 0.0031308 {
        x * 12.92
    } else {
        1.055 * x.max(0.).powf(1. / 2.4) - 0.055
    }
}
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn lum(c: Rgb, w: Rgb) -> f32 {
    (0..3).map(|i| c[i] * w[i]).sum()
}
fn set_lum(mut c: Rgb, l: f32) -> Rgb {
    let d = l - lum(c, [0.30, 0.59, 0.11]);
    for x in &mut c {
        *x += d;
    }
    let lo = c.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = c.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if lo < 0. {
        c = c.map(|x| l + (x - l) * l / (l - lo));
    }
    if hi > 1. {
        c = c.map(|x| l + (x - l) * (1. - l) / (hi - l));
    }
    c
}
fn ramp(p: &RenderParams, mut t: f32, range: [f64; 2], numeric: bool) -> Rgb {
    if p.ramp_range_mode == "custom" {
        let q = if numeric {
            ((t - 1e-6) / (1. - 1e-6)).clamp(0., 1.)
        } else {
            t
        };
        t = ((range[0] - p.ramp_min + q as f64 * range[1]) / (p.ramp_max - p.ramp_min).max(1e-12))
            as f32;
    }
    if p.ramp_reverse {
        t = 1. - t;
    }
    t -= p.ramp_offset;
    t = if p.ramp_wrap {
        t - t.floor()
    } else {
        t.clamp(0., 1.)
    };
    let discrete = p.ramp_interpolation == "discrete";
    if p.ramp_stops.len() >= 2 {
        let stops = &p.ramp_stops;
        if t <= stops[0].position {
            return rgb(&stops[0].color);
        }
        for s in stops.windows(2) {
            if t < s[1].position {
                return if discrete {
                    rgb(&s[0].color)
                } else {
                    mix(
                        rgb(&s[0].color),
                        rgb(&s[1].color),
                        (t - s[0].position) / (s[1].position - s[0].position).max(1e-12),
                    )
                };
            }
        }
        return rgb(&stops.last().unwrap().color);
    }
    if p.color_ramp == "grayscale" {
        return [if discrete {
            (t * 5.).floor().min(4.) / 4.
        } else {
            t
        }; 3];
    }
    let colors = match p.color_ramp.as_str() {
        "custom" => p.ramp_colors.each_ref().map(|s| rgb(s)),
        "spectral" => [
            [0.20, 0.25, 0.65],
            [0.15, 0.65, 0.78],
            [0.65, 0.82, 0.38],
            [0.98, 0.72, 0.22],
            [0.75, 0.16, 0.20],
        ],
        _ => [
            [0.15, 0.32, 0.23],
            [0.43, 0.57, 0.32],
            [0.70, 0.64, 0.43],
            [0.61, 0.52, 0.43],
            [0.94, 0.93, 0.89],
        ],
    };
    if discrete {
        return colors[(t * 5.).floor().min(4.) as usize];
    }
    let q = t * 4.;
    let i = (q.floor() as usize).min(3);
    mix(colors[i], colors[i + 1], q - i as f32)
}
fn tex_sample(p: &RenderParams, tex: &ColorTexture, st: [f32; 2], extent: [f32; 2]) -> [f32; 4] {
    let mut q = [st[0] - 0.5, st[1] - 0.5];
    if p.texture_fit != "fill" {
        let plane = extent[0].max(1e-8) / extent[1].max(1e-8);
        let image = tex.width as f32 / tex.height.max(1) as f32;
        q[0] *= (plane / image).max(1.);
        q[1] *= (image / plane).max(1.);
    }
    let (s, c) = p.texture_rotation.to_radians().sin_cos();
    let scale = p.texture_scale.max(1e-6);
    q = [
        (c * q[0] + s * q[1]) / scale + 0.5 - p.texture_offset_x,
        (-s * q[0] + c * q[1]) / scale + 0.5 - p.texture_offset_y,
    ];
    let repeat = p.texture_fit == "repeat";
    if !repeat && q.iter().any(|x| *x < 0. || *x > 1.) {
        return [0.; 4];
    }
    let size = [tex.width as i32, tex.height as i32];
    if size.iter().any(|x| *x <= 0) {
        return [0.; 4];
    }
    // Uploaded texture rows are reversed. Read decoder rows with exactly one flip.
    let pixel = std::array::from_fn::<_, 2, _>(|i| {
        if repeat {
            (q[i] - q[i].floor()) * size[i] as f32 - 0.5
        } else {
            q[i] * (size[i] - 1) as f32
        }
    });
    let lo = pixel.map(|x| x.floor() as i32);
    let f = std::array::from_fn::<_, 2, _>(|i| pixel[i] - lo[i] as f32);
    let at = |x: i32, y: i32| {
        let clamp = |v: i32, n: i32| {
            if repeat {
                v.rem_euclid(n)
            } else {
                v.clamp(0, n - 1)
            }
        };
        let ix = clamp(x, size[0]);
        let iy = size[1] - 1 - clamp(y, size[1]);
        let n = ((iy * size[0] + ix) * 4) as usize;
        std::array::from_fn::<_, 4, _>(|i| {
            let v = tex.rgba.get(n + i).copied().unwrap_or(0) as f32 / 255.;
            if i < 3 {
                linear(v)
            } else {
                v
            }
        })
    };
    let a = at(lo[0], lo[1]);
    let b = at(lo[0] + 1, lo[1]);
    let c = at(lo[0], lo[1] + 1);
    let d = at(lo[0] + 1, lo[1] + 1);
    std::array::from_fn(|i| {
        (a[i] * (1. - f[0]) + b[i] * f[0]) * (1. - f[1]) + (c[i] * (1. - f[0]) + d[i] * f[0]) * f[1]
    })
}
fn layer(p: &RenderParams, tex: &ColorTexture, uv: [f32; 2], h: f32, n: Rgb, g: Rgb) -> [f32; 4] {
    let st = [uv[0], 1. - uv[1]];
    let g = g.map(|x| x.max(1e-8));
    if p.texture_mapping == "aligned" {
        return tex_sample(p, tex, st, [g[0], g[1]]);
    }
    if p.texture_mapping != "triplanar" {
        return tex_sample(
            p,
            tex,
            [(st[0] - 0.5) * g[0] + 0.5, (st[1] - 0.5) * g[1] + 0.5],
            [1.; 2],
        );
    }
    let q = ((h - 1e-6) / (1. - 1e-6)).clamp(0., 1.);
    let curve = if p.height_scale_mode == "nonlinear" {
        p.height_curve.max(1e-6)
    } else {
        1.
    };
    let height = q / (curve * (1. - q) + q) * (1. - 1e-6) + 1e-6;
    let pos = [
        (st[0] - 0.5) * g[0],
        (st[1] - 0.5) * g[1],
        (height - 0.5) * g[2],
    ];
    let mut w = n.map(|x| x.abs().powi(4));
    let sum = w.iter().sum::<f32>();
    w = if sum > 1e-20 {
        w.map(|x| x / sum)
    } else {
        [0., 0., 1.]
    };
    let samples = [
        tex_sample(p, tex, [pos[1] + 0.5, pos[2] + 0.5], [1.; 2]),
        tex_sample(p, tex, [pos[0] + 0.5, pos[2] + 0.5], [1.; 2]),
        tex_sample(p, tex, [pos[0] + 0.5, pos[1] + 0.5], [1.; 2]),
    ];
    let a = (0..3).map(|i| samples[i][3] * w[i]).sum::<f32>();
    let c = std::array::from_fn::<_, 3, _>(|k| {
        if a > 0. {
            (0..3)
                .map(|i| samples[i][k] * samples[i][3] * w[i])
                .sum::<f32>()
                / a
        } else {
            0.
        }
    });
    [c[0], c[1], c[2], a]
}
/// Returns opaque, unlit display RGB. No sun, sky, shadows, fog or tone mapping.
pub(super) fn color(
    p: &RenderParams,
    tex: Option<&ColorTexture>,
    uv: [f32; 2],
    height: f32,
    normal: Rgb,
    geometry: Rgb,
    source_elevation: [f64; 2],
    numeric: bool,
) -> [u8; 4] {
    let independent = p.appearance_layers_version >= 1;
    let elevation = if independent {
        p.elevation_enabled
    } else {
        p.appearance_mode == "elevation"
    };
    let texture = if independent {
        p.texture_enabled
    } else {
        p.appearance_mode == "texture"
    };
    let material = !independent || p.material_enabled;
    let default = rgb("#d5d2c9");
    let mut base = if material {
        if !independent && elevation {
            rgb(&p.material_color)
        } else {
            mix(
                default,
                rgb(&p.material_color),
                (p.material_opacity / 100.).clamp(0., 1.),
            )
        }
    } else {
        default
    };
    if elevation {
        let c = ramp(p, height, source_elevation, numeric);
        let opacity = (p.elevation_opacity / 100.).clamp(0., 1.);
        base = if p.elevation_blend == "color" {
            mix(base, set_lum(c, lum(base, [0.30, 0.59, 0.11])), opacity)
        } else {
            mix(base.map(linear), c.map(linear), opacity).map(srgb)
        };
    }
    if texture {
        if let Some(tex) = tex {
            let sample = layer(p, tex, uv, height, normal, geometry);
            let mut c = [sample[0], sample[1], sample[2]].map(srgb);
            c = c.map(|x| x * 2f32.powf(p.texture_exposure));
            let l = lum(c, [0.2126, 0.7152, 0.0722]);
            let shift = (p.texture_shadows / 100. * (1. - smooth(0., 0.65, l))
                + p.texture_highlights / 100. * smooth(0.35, 1., l))
                * 0.35;
            c = c.map(|x| (x + shift - 0.5) * 2f32.powf(p.texture_contrast / 100.) + 0.5);
            let l = lum(c, [0.2126, 0.7152, 0.0722]);
            c = c.map(|x| (l + (x - l) * (1. + p.texture_saturation / 100.)).clamp(0., 1.));
            let blend = std::array::from_fn(|i| {
                let b = base[i];
                let c = c[i];
                match p.texture_blend.as_str() {
                    "normal" => c,
                    "multiply" => b * c,
                    "softlight" => {
                        let d = if b < 0.25 {
                            ((16. * b - 12.) * b + 4.) * b
                        } else {
                            b.max(0.).sqrt()
                        };
                        if c < 0.5 {
                            b - (1. - 2. * c) * b * (1. - b)
                        } else {
                            b + (2. * c - 1.) * (d - b)
                        }
                    }
                    _ => {
                        if b < 0.5 {
                            2. * b * c
                        } else {
                            1. - 2. * (1. - b) * (1. - c)
                        }
                    }
                }
            });
            base = mix(
                base,
                blend,
                (sample[3] * p.texture_opacity / 100.).clamp(0., 1.),
            );
        }
    }
    let c = base.map(|x| (x.clamp(0., 1.) * 255.).round() as u8);
    [c[0], c[1], c[2], 255]
}

//! Cached, sun-relative atmosphere radiance for the path tracer.
//!
//! Ports the spherical exponential-density Rayleigh/Mie single-scattering
//! model in research/shaders/07_atmosphere_lighting.glsl. Integrating here
//! avoids its 16 x 8 density evaluations on every escaping path. Intensity,
//! tint and azimuth are applied by the renderer and do not invalidate the LUT.

use std::f64::consts::PI;

pub const SKY_WIDTH: usize = 128;
pub const SKY_HEIGHT: usize = 64;
// Distances are expressed in 8 km density scale heights.
const EARTH_RADIUS: f64 = 6371.0 / 8.0;
const TOP_RADIUS: f64 = EARTH_RADIUS + 10.0;
const RAYLEIGH: [f64; 3] = [0.0464, 0.108, 0.2648];
const MIE: f64 = 0.016;
const EXTINCTION_MIE: f64 = MIE * 1.11;
const G: f64 = 0.76;

pub struct SkyLut {
    /// Cache key; altitude in degrees. Sun azimuth is a sampling rotation.
    pub altitude: f32,
    /// RGB32F, row-major, using the existing uv_to_dir mapping: south pole at
    /// v=0, north pole at v=1, solar azimuth zero at u=0.5.
    pub pixels: Vec<f32>,
    pub mean_rgb: [f32; 3],
}

impl SkyLut {
    pub fn new(sun_altitude: f32) -> Self {
        let altitude = valid_altitude(sun_altitude);
        let a = f64::from(altitude).to_radians();
        let sun = [a.cos(), 0.0, a.sin()];
        let mut pixels = vec![0.0; SKY_WIDTH * SKY_HEIGHT * 3];
        let mut irradiance = 0.0;
        let mut weights = 0.0;
        let mut rgb_sum = [0.; 3];
        for y in 0..SKY_HEIGHT {
            let theta = (1.0 - (y as f64 + 0.5) / SKY_HEIGHT as f64) * PI;
            let z = theta.cos();
            if z <= 0.0 {
                continue;
            }
            let weight = z * theta.sin();
            for x in 0..SKY_WIDTH {
                let phi = (1.0 - (x as f64 + 0.5) / SKY_WIDTH as f64) * PI * 2.0 - PI;
                let dir = [phi.cos() * theta.sin(), phi.sin() * theta.sin(), z];
                let rgb = in_scatter(dir, sun);
                let at = (y * SKY_WIDTH + x) * 3;
                for channel in 0..3 {
                    pixels[at + channel] = rgb[channel] as f32;
                    rgb_sum[channel] += rgb[channel] * weight;
                }
                irradiance += luminance(rgb) * weight;
                weights += weight;
            }
        }
        // A flat Lambertian receiver sees the same mean luminance as a unit
        // constant sky, while angular structure and atmospheric color remain.
        let mean = irradiance / weights;
        if mean.is_finite() && mean > 0.0 {
            for value in &mut pixels {
                *value /= mean as f32;
            }
        }
        let mean_rgb=std::array::from_fn(|i| (rgb_sum[i]/weights/mean) as f32);
        Self { altitude, pixels, mean_rgb }
    }
}

/// Mean positive elevation cosine of the uniformly sampled solar cone.
pub fn sun_mean_cosine(altitude: f32, diameter: f32) -> f32 {
    let a=f64::from(altitude).to_radians();
    let angle=f64::from(diameter.clamp(0.01,90.)).to_radians()*0.5;
    let c=angle.cos();
    if a>=angle { return (a.sin()*(1.+c)*0.5) as f32; }
    let mut sum=0.;
    for ring in 0..16 {
        let z=1.-(1.-c)*(ring as f64+0.5)/16.;
        let r=(1.-z*z).sqrt();
        for sector in 0..32 {
            let phi=(sector as f64+0.5)*2.*PI/32.;
            sum+=(z*a.sin()+r*phi.cos()*a.cos()).max(0.);
        }
    }
    (sum/512.) as f32
}

/// Atmospheric attenuation of the direct solar beam, independent of the
/// user's sky intensity. No solar disk is baked into the sky texture.
pub fn direct_sun_transmittance(sun_altitude: f32) -> [f32; 3] {
    let a = f64::from(valid_altitude(sun_altitude)).to_radians();
    let dir = [a.cos(), 0.0, a.sin()];
    let origin = [0.0, 0.0, EARTH_RADIUS];
    let depth = optical_depth(origin, dir, sphere_exit(origin, dir));
    std::array::from_fn(|channel| (-depth * (RAYLEIGH[channel] + EXTINCTION_MIE)).exp() as f32)
}

fn valid_altitude(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 90.0)
    } else {
        45.0
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn along(origin: [f64; 3], dir: [f64; 3], distance: f64) -> [f64; 3] {
    std::array::from_fn(|i| origin[i] + dir[i] * distance)
}

fn density(point: [f64; 3]) -> f64 {
    (-(dot(point, point).sqrt() - EARTH_RADIUS).max(0.0)).exp()
}

fn sphere_exit(origin: [f64; 3], dir: [f64; 3]) -> f64 {
    let b = dot(origin, dir);
    let c = dot(origin, origin) - TOP_RADIUS * TOP_RADIUS;
    (-b + (b * b - c).max(0.0).sqrt()).max(0.0)
}

fn optical_depth(origin: [f64; 3], dir: [f64; 3], distance: f64) -> f64 {
    let step = distance / 8.0;
    (0..8)
        .map(|i| density(along(origin, dir, (i as f64 + 0.5) * step)))
        .sum::<f64>()
        * step
}

fn in_scatter(dir: [f64; 3], sun: [f64; 3]) -> [f64; 3] {
    let origin = [0.0, 0.0, EARTH_RADIUS];
    let step = sphere_exit(origin, dir) / 16.0;
    let mut view_depth = 0.0;
    let mut sum = [0.0; 3];
    for i in 0..16 {
        let point = along(origin, dir, (i as f64 + 0.5) * step);
        let den = density(point);
        let sun_depth = optical_depth(point, sun, sphere_exit(point, sun));
        let depth = view_depth + den * step * 0.5 + sun_depth;
        for channel in 0..3 {
            sum[channel] += den * (-depth * (RAYLEIGH[channel] + EXTINCTION_MIE)).exp();
        }
        view_depth += den * step;
    }
    let cosine = dot(dir, sun).clamp(-1.0, 1.0);
    let cc = cosine * cosine;
    let ray_phase = 3.0 / (16.0 * PI) * (1.0 + cc);
    let mie_phase = 3.0 / (8.0 * PI) * (1.0 - G * G) * (1.0 + cc)
        / ((2.0 + G * G) * (1.0 + G * G - 2.0 * G * cosine).powf(1.5));
    std::array::from_fn(|channel| {
        sum[channel] * step * (RAYLEIGH[channel] * ray_phase + MIE * mie_phase)
    })
}

fn luminance(rgb: [f64; 3]) -> f64 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

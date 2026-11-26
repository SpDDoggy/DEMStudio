//! Local Z-up terrain mesh export; curved surfaces are tessellated approximations.
use super::{appearance::ColorTexture, HeightMap, RenderParams};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
type R<T> = Result<T, String>;
#[derive(Clone, Copy)]
struct Vertex {
    p: [f32; 3],
    n: [f32; 3],
    uv: [f32; 2],
    h: f32,
}
struct Grid {
    w: usize,
    h: usize,
    z: Vec<f32>,
    curve: f32,
    scale: f32,
    offset: f32,
}
impl Grid {

    fn prepare(map: &HeightMap, p: &RenderParams, progress: &impl Fn(u32) -> R<()>) -> R<Self> {
        if !["cube", "poly", "bilinear", "bicubic"].contains(&p.surface.as_str())
            || !["linear", "nonlinear", "accelerated"].contains(&p.height_scale_mode.as_str())
            || !p.height_scale.is_finite()
            || p.height_scale <= 0.
            || !p.height_curve.is_finite()
            || p.height_curve <= 0.
            || !p.height_offset.is_finite()
            || !p.height_step.is_finite()
            || p.height_step < 0.
            || map.values.iter().any(|v| !v.is_finite())
            || map.display_range.iter().any(|v| !v.is_finite())
        {
            return Err("模型参数无效".into());
        }
        let cells = (map.width as usize)
            .checked_mul(map.height as usize)
            .ok_or("模型尺寸溢出")?;
        let refined = super::bilinear_refine::active(p);
        if cells
            .checked_mul(if refined { 4 } else { 1 })
            .filter(|&v| v <= 16_777_216)
            .is_none()
        {
            return Err("模型网格过大，请降低精度".into());
        }
        if map.width < 2
            || map.height < 2
            || map.values.len() != map.width as usize * map.height as usize
        {
            return Err("地形网格尺寸无效".into());
        }
        let raw: Vec<f32> = map
            .values
            .iter()
            .map(|&v| super::gl_renderer::display_sample(v, map.display_range))
            .collect();
        let curve = if p.height_scale_mode == "nonlinear" {
            p.height_curve
        } else {
            1.
        };
        let mut z: Vec<f32> = raw
            .iter()
            .map(|&v| {
                if v == 0. {
                    0.
                } else {
                    let q = ((v - 1e-6) / (1. - 1e-6)).clamp(0., 1.);
                    q / (curve * (1. - q) + q) * (1. - 1e-6) + 1e-6
                }
            })
            .collect();
        let (mut w, mut h) = (map.width as usize, map.height as usize);
        let refined = super::bilinear_refine::active(p);

        if refined {
            let smooth = refined && super::bilinear_refine::bspline_enabled();
            let refine = |data: &[f32]| -> R<Vec<f32>> {
                let mut out = vec![0.; w * h * 4];
                for y in 0..h * 2 {
                    if y % 16 == 0 {
                        progress(1)?;
                    }
                    for x in 0..w * 2 {
                        let px = x as f32 * 0.5 - 0.25;
                        let py = y as f32 * 0.5 - 0.25;
                        let ix = px.floor() as isize;
                        let iy = py.floor() as isize;
                        let mut v = [[0.; 4]; 4];
                        for j in 0..4 {
                            for i in 0..4 {
                                let xx = (ix + i as isize - 1).clamp(0, w as isize - 1) as usize;
                                let yy = (iy + j as isize - 1).clamp(0, h as isize - 1) as usize;
                                v[j][i] = data[yy * w + xx];
                            }
                        }
                        out[y * w * 2 + x] = super::bilinear_refine::patch_with_basis(
                            v,
                            px - px.floor(),
                            py - py.floor(),
                            smooth,
                        );
                    }
                }
                Ok(out)
            };
            z = refine(&z)?;

            w *= 2;
            h *= 2;
        }
        // Renderer height decoding quantizes before adding the offset.
        let longest = map.width.max(map.height) as f32;
        let scale = 0.2 * p.height_scale;
        if p.height_step > 0. {
            let step = p.height_step * p.height_scale / longest;
            for v in &mut z {
                if *v > 0. {
                    *v = (*v * scale / step).ceil() * step / scale;
                }
            }
        }
        Ok(Self {
            w,
            h,
            z,
            curve,
            scale,
            offset: p.height_offset * 0.01,
        })
    }
    fn inverse_height(&self, z: f32) -> f32 {
        let t = (((z - self.offset) / self.scale - 1e-6) / (1. - 1e-6)).clamp(0., 1.);
        let q = self.curve * t / (1. + (self.curve - 1.) * t);
        q * (1. - 1e-6) + 1e-6
    }
    fn located(&self, mut v: Vertex) -> Vertex {
        let l = self.w.max(self.h) as f32;
        v.h = self.inverse_height(v.p[2]);
        v.uv = [
            v.p[0] * l / (self.w - 1) as f32,
            1. - v.p[1] * l / (self.h - 1) as f32,
        ];
        v
    }
    fn sample(&self, u: f32, v: f32) -> Vertex {
        let x = u * (self.w - 1) as f32;
        let y = v * (self.h - 1) as f32;
        let ix = (x.floor() as usize).min(self.w - 2);
        let iy = (y.floor() as usize).min(self.h - 2);
        let tx = x - ix as f32;
        let ty = y - iy as f32;
        let a = self.point(ix, iy);
        let b = self.point(ix + 1, iy);
        let c = self.point(ix, iy + 1);
        let d = self.point(ix + 1, iy + 1);
        let mut out = a;
        for k in 0..3 {
            out.p[k] = mix(mix(a.p[k], b.p[k], tx), mix(c.p[k], d.p[k], tx), ty);
            out.n[k] = mix(mix(a.n[k], b.n[k], tx), mix(c.n[k], d.n[k], tx), ty);
        }
        let n = out.n.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        out.n = out.n.map(|x| x / n);
        self.located(out)
    }
    fn point(&self, x: usize, y: usize) -> Vertex {
        let i = y * self.w + x;
        let len = self.w.max(self.h) as f32;
        let z = self.z[i] * self.scale + self.offset;
        let sample = |xx: usize, yy: usize| {
            let v = self.z[yy * self.w + xx];
            if v == 0. {
                self.z[i]
            } else {
                v
            }
        };
        let xl = x.saturating_sub(1);
        let xr = (x + 1).min(self.w - 1);
        let yt = y.saturating_sub(1);
        let yb = (y + 1).min(self.h - 1);
        let dx = (sample(xr, y) - sample(xl, y)) * self.scale * len / (xr - xl).max(1) as f32;
        let dy = (sample(x, yt) - sample(x, yb)) * self.scale * len / (yb - yt).max(1) as f32;
        let norm = (dx * dx + dy * dy + 1.).sqrt();
        Vertex {
            p: [x as f32 / len, (self.h - 1 - y) as f32 / len, z],
            n: [-dx / norm, -dy / norm, 1. / norm],
            uv: [
                x as f32 / (self.w - 1) as f32,
                y as f32 / (self.h - 1) as f32,
            ],
            h: self.inverse_height(z),
        }
    }
    fn faces(&self, mut emit: impl FnMut([u32; 3]) -> R<()>) -> R<()> {
        for y in 0..self.h - 1 {
            for x in 0..self.w - 1 {
                let a = y * self.w + x;
                let b = a + 1;
                let c = a + self.w;
                let d = c + 1;
                for t in [[a, c, b], [b, c, d]] {
                    if t.iter().all(|&i| self.z[i] > 0.) {
                        emit(t.map(|i| i as u32))?;
                    }
                }
            }
        }
        Ok(())
    }
    fn wall_quads(&self, mut emit: impl FnMut([Vertex; 4]) -> R<()>) -> R<()> {
        self.faces(|tri| {
            for k in 0..3 {
                let a = tri[k] as usize;
                let b = tri[(k + 1) % 3] as usize;
                let ax = a % self.w;
                let ay = a / self.w;
                let bx = b % self.w;
                let by = b / self.w;
                let mut adjacent = 0;
                for cy in ay.min(by).saturating_sub(1)..=ay.max(by).min(self.h - 2) {
                    for cx in ax.min(bx).saturating_sub(1)..=ax.max(bx).min(self.w - 2) {
                        let i = cy * self.w + cx;
                        for t in [[i, i + self.w, i + 1], [i + 1, i + self.w, i + self.w + 1]] {
                            if t.contains(&a) && t.contains(&b) && t.iter().all(|&j| self.z[j] > 0.)
                            {
                                adjacent += 1;
                            }
                        }
                    }
                }
                if adjacent == 1 {
                    let va = self.point(ax, ay);
                    let vb = self.point(bx, by);
                    if va.p[2] <= 0. && vb.p[2] <= 0. {
                        continue;
                    }
                    let dx = vb.p[0] - va.p[0];
                    let dy = vb.p[1] - va.p[1];
                    let l = dx.hypot(dy);
                    let n = [dy / l, -dx / l, 0.];
                    let mut ba = va;
                    ba.p[2] = 0.;
                    let mut bb = vb;
                    bb.p[2] = 0.;
                    emit(
                        [
                            Vertex { n, ..ba },
                            Vertex { n, ..bb },
                            Vertex { n, ..vb },
                            Vertex { n, ..va },
                        ]
                        .map(|v| self.located(v)),
                    )?;
                }
            }
            Ok(())
        })
    }
    fn cube_quads(&self, mut emit: impl FnMut([Vertex; 4]) -> R<()>) -> R<()> {
        let l = self.w.max(self.h) as f32;
        for y in 0..self.h {
            for x in 0..self.w {
                let v = self.point(x, y);
                if self.z[y * self.w + x] == 0. {
                    continue;
                }
                let x0 = x as f32 / l;
                let x1 = (x + 1) as f32 / l;
                let y0 = (self.h - 1 - y) as f32 / l;
                let y1 = y0 + 1. / l;
                let z = v.p[2];
                let mut quad = |pts: [[f32; 3]; 4], n: [f32; 3]| {
                    emit(pts.map(|p| self.located(Vertex { p, n, ..v })))
                };
                quad(
                    [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]],
                    [0., 0., 1.],
                )?;
                for (dx, dy, edge, n) in [
                    (-1, 0, [[x0, y1], [x0, y0]], [-1., 0., 0.]),
                    (1, 0, [[x1, y0], [x1, y1]], [1., 0., 0.]),
                    (0, -1, [[x1, y1], [x0, y1]], [0., 1., 0.]),
                    (0, 1, [[x0, y0], [x1, y0]], [0., -1., 0.]),
                ] {
                    let xx = x as isize + dx;
                    let yy = y as isize + dy;
                    let low = if xx < 0 || yy < 0 || xx >= self.w as isize || yy >= self.h as isize
                    {
                        0.
                    } else {
                        let i = yy as usize * self.w + xx as usize;
                        if self.z[i] == 0. {
                            0.
                        } else {
                            self.z[i] * self.scale + self.offset
                        }
                    };
                    if low < z {
                        quad(
                            [
                                [edge[0][0], edge[0][1], low],
                                [edge[1][0], edge[1][1], low],
                                [edge[1][0], edge[1][1], z],
                                [edge[0][0], edge[0][1], z],
                            ],
                            n,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}
struct Atlas {
    width: u32,
    height: u32,
    columns: u32,
}
impl Atlas {
    fn new(quads: u64) -> R<Self> {
        if quads > 1_048_576 {
            return Err("OBJ侧面纹理超过4096×4096，请降低模型精度".into());
        }
        let columns = (quads.max(1) as f64).sqrt().ceil() as u32;
        let rows = ((quads.max(1) + columns as u64 - 1) / columns as u64) as u32;
        Ok(Self {
            width: columns * 4,
            height: rows * 4,
            columns,
        })
    }
    fn pixel(&self, q: u64, x: u32, y: u32) -> (u32, u32) {
        (
            (q as u32 % self.columns) * 4 + x,
            (q as u32 / self.columns) * 4 + y,
        )
    }
    fn uv(&self, q: u64, k: usize) -> [f32; 2] {
        let (x, y) = match k {
            0 => (0, 0),
            1 => (3, 0),
            2 => (3, 3),
            _ => (0, 3),
        };
        let (px, py) = self.pixel(q, x, y);
        [
            (px as f32 + 0.5) / self.width as f32,
            (py as f32 + 0.5) / self.height as f32,
        ]
    }
}
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn io(e: std::io::Error) -> String {
    e.to_string()
}
fn temp(path: &Path, tag: &str) -> R<(PathBuf, File)> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let name = format!("demstudio-{stamp}-{tag}");
    let target = path.with_file_name(name);
    let f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(io)?;
    Ok((target, f))
}
/// The caller authorizes replacement of `path`; all sidecars receive unique names.
pub fn export(
    path: &Path,
    map: &HeightMap,
    p: &RenderParams,
    texture: Option<&ColorTexture>,
    progress: impl Fn(u32) -> R<()>,
) -> R<()> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "obj" && ext != "ply" {
        return Err("模型格式仅支持 PLY、OBJ".into());
    }
    progress(0)?;
    let g = Grid::prepare(map, p, &progress)?;
    let cube = p.surface == "cube";
    let mut quads = 0u64;
    if cube {
        g.cube_quads(|_| {
            quads += 1;
            if quads % 4096 == 0 {
                progress(2)?;
            }
            Ok(())
        })?;
    } else {
        g.wall_quads(|_| {
            quads += 1;
            if quads % 4096 == 0 {
                progress(2)?;
            }
            Ok(())
        })?;
    }
    let vertices = if cube {
        quads * 4
    } else {
        (g.w * g.h) as u64 + quads * 4
    };
    if vertices > u32::MAX as u64 {
        return Err("模型顶点数超过格式上限".into());
    }
    let mut faces = quads * 2;
    if !cube {
        g.faces(|_| {
            faces += 1;
            if faces % 4096 == 0 {
                progress(3)?;
            }
            Ok(())
        })?;
    }
    let (mut files, mut committed) = (Vec::<PathBuf>::new(), false);
    let atlas = Atlas::new(if ext == "obj" { quads } else { 0 })?;
    let result = (|| {
        let (tmp, file) = temp(path, "mesh.tmp")?;
        files.push(tmp.clone());
        let mut out = BufWriter::new(file);
        let mut sidecars = None;
        if ext == "obj" {
            let (mtl, mfile) = temp(path, "material.mtl")?;
            files.push(mtl.clone());
            let (png, pfile) = temp(path, "albedo.png")?;
            files.push(png.clone());
            drop(pfile);
            let tw =
                g.w.max(texture.map(|t| t.width as usize).unwrap_or(0))
                    .min(4096) as u32;
            let th =
                g.h.max(texture.map(|t| t.height as usize).unwrap_or(0))
                    .min(4096) as u32;
            let mut pixels = Vec::with_capacity((tw * th * 4) as usize);
            for y in 0..th {
                for x in 0..tw {
                    let v = g.sample(
                        x as f32 / (tw - 1).max(1) as f32,
                        y as f32 / (th - 1).max(1) as f32,
                    );
                    pixels.extend(color(map, p, texture, &g, v));
                }
                progress(5 + (y * 20 / th))?;
            }
            image::save_buffer_with_format(
                &png,
                &pixels,
                tw,
                th,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
            let (sidepng, sidefile) = temp(path, "sides.png")?;
            files.push(sidepng.clone());
            drop(sidefile);
            let mut sidepixels = vec![0u8; (atlas.width * atlas.height * 4) as usize];
            let mut qi = 0u64;
            let mut bake = |q: [Vertex; 4]| -> R<()> {
                for y in 0..4 {
                    for x in 0..4 {
                        let u = x as f32 / 3.;
                        let v = y as f32 / 3.;
                        let mut vertex = q[0];
                        for k in 0..3 {
                            vertex.p[k] = mix(
                                mix(q[0].p[k], q[1].p[k], u),
                                mix(q[3].p[k], q[2].p[k], u),
                                v,
                            );
                        }
                        let pixel = color(map, p, texture, &g, vertex);
                        let (px, py) = atlas.pixel(qi, x, y);
                        let offset = ((py * atlas.width + px) * 4) as usize;
                        sidepixels[offset..offset + 4].copy_from_slice(&pixel);
                    }
                }
                qi += 1;
                if qi % 4096 == 0 {
                    progress(24)?;
                }
                Ok(())
            };
            if cube {
                g.cube_quads(&mut bake)?;
            } else {
                g.wall_quads(&mut bake)?;
            }
            image::save_buffer_with_format(
                &sidepng,
                &sidepixels,
                atlas.width,
                atlas.height,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
            let mname = mtl.file_name().unwrap().to_string_lossy();
            let pname = png.file_name().unwrap().to_string_lossy();
            let mut mw = BufWriter::new(mfile);
            write!(
                mw,
                "newmtl terrain\nKd 1 1 1\nKa 0 0 0\nKs 0 0 0\nd 1\nillum 1\nmap_Kd {pname}\n"
            )
            .map_err(io)?;
            writeln!(
                mw,
                "newmtl terrain_sides\nKd 1 1 1\nKa 0 0 0\nKs 0 0 0\nd 1\nillum 1\nmap_Kd {}",
                sidepng.file_name().unwrap().to_string_lossy()
            )
            .map_err(io)?;
            mw.flush().map_err(io)?;
            writeln!(
                out,
                "# DEMStudio local Z-up tessellated terrain\nmtllib {mname}\nusemtl terrain"
            )
            .map_err(io)?;
            sidecars = Some((mtl, png));
        } else {
            write!(out,"ply\nformat binary_little_endian 1.0\ncomment DEMStudio local Z-up tessellated terrain\nelement vertex {vertices}\nproperty float x\nproperty float y\nproperty float z\nproperty float nx\nproperty float ny\nproperty float nz\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nproperty uchar alpha\nelement face {faces}\nproperty list uchar uint vertex_indices\nend_header\n").map_err(io)?;
        }
        let mut count = 0u64;
        let mut emit = |v: Vertex| -> R<()> {
            if ext == "ply" {
                for f in v.p.into_iter().chain(v.n) {
                    out.write_all(&f.to_le_bytes()).map_err(io)?;
                }
                out.write_all(&color(map, p, texture, &g, v)).map_err(io)?;
            } else {
                writeln!(out, "v {} {} {}", v.p[0], v.p[1], v.p[2]).map_err(io)?;
            }
            count += 1;
            if count % 4096 == 0 {
                progress(25 + (count * 35 / vertices.max(1)) as u32)?;
            }
            Ok(())
        };
        if cube {
            g.cube_quads(|q| {
                for v in q {
                    emit(v)?;
                }
                Ok(())
            })?;
        } else {
            for y in 0..g.h {
                for x in 0..g.w {
                    emit(g.point(x, y))?;
                }
            }
            g.wall_quads(|q| {
                for v in q {
                    emit(v)?;
                }
                Ok(())
            })?;
        }
        drop(emit);
        if ext == "obj" {
            for attr in 0..2 {
                let mut seen = 0u64;
                let mut emit = |v: Vertex| -> R<()> {
                    seen += 1;
                    if seen % 4096 == 0 {
                        progress(60)?;
                    }
                    if attr == 0 {
                        writeln!(out, "vt {} {}", v.uv[0], 1. - v.uv[1]).map_err(io)
                    } else {
                        writeln!(out, "vn {} {} {}", v.n[0], v.n[1], v.n[2]).map_err(io)
                    }
                };
                if !cube {
                    for y in 0..g.h {
                        for x in 0..g.w {
                            emit(g.point(x, y))?;
                        }
                    }
                }
                let mut qi = 0u64;
                let mut quad = |q: [Vertex; 4]| -> R<()> {
                    for (k, mut v) in q.into_iter().enumerate() {
                        if attr == 0 {
                            v.uv = atlas.uv(qi, k);
                        }
                        emit(v)?;
                    }
                    qi += 1;
                    Ok(())
                };
                if cube {
                    g.cube_quads(&mut quad)?;
                } else {
                    g.wall_quads(&mut quad)?;
                }
            }
        }
        let mut done = 0u64;
        let mut side_material = false;
        let mut face = |t: [u32; 3]| -> R<()> {
            if ext == "ply" {
                out.write_all(&[3]).map_err(io)?;
                for i in t {
                    out.write_all(&i.to_le_bytes()).map_err(io)?;
                }
            } else {
                if !side_material && (cube || t[0] >= (g.w * g.h) as u32) {
                    writeln!(out, "usemtl terrain_sides").map_err(io)?;
                    side_material = true;
                }
                let [a, b, c] = t.map(|i| i + 1);
                writeln!(out, "f {a}/{a}/{a} {b}/{b}/{b} {c}/{c}/{c}").map_err(io)?;
            }
            done += 1;
            if done % 4096 == 0 {
                progress(60 + (done * 38 / faces.max(1)) as u32)?;
            }
            Ok(())
        };
        if cube {
            for q in 0..quads {
                let i = (q * 4) as u32;
                face([i, i + 1, i + 2])?;
                face([i, i + 2, i + 3])?;
            }
        } else {
            g.faces(&mut face)?;
            let base = (g.w * g.h) as u32;
            for q in 0..quads {
                let i = base + (q * 4) as u32;
                face([i, i + 1, i + 2])?;
                face([i, i + 2, i + 3])?;
            }
        }
        drop(face);
        out.flush().map_err(io)?;
        out.get_ref().sync_all().map_err(io)?;
        drop(out);
        progress(99)?;
        // Windows rename cannot replace; use platform atomic replacement for an existing target.
        if path.exists() {
            replace(&tmp, path).map_err(io)?;
        } else {
            fs::rename(&tmp, path).map_err(io)?;
        }
        committed = true;
        let _ = sidecars;
        Ok(())
    })();
    if !committed {
        for f in files {
            let _ = fs::remove_file(f);
        }
    }
    result
}
fn color(
    map: &HeightMap,
    p: &RenderParams,
    texture: Option<&ColorTexture>,
    g: &Grid,
    v: Vertex,
) -> [u8; 4] {
    let l = g.w.max(g.h).saturating_sub(1).max(1) as f32;
    let v = g.located(v);
    let source = super::gl_renderer::source_elevation_range(map);
    let numeric = map.source_elevation_range.is_some()
        && map
            .source
            .as_ref()
            .and_then(|s| s.path.extension())
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("tif") || s.eq_ignore_ascii_case("tiff"));
    super::mesh_appearance::color(
        p,
        texture,
        v.uv,
        v.h,
        v.n,
        [
            (g.w - 1) as f32 / l,
            (g.h - 1) as f32 / l,
            g.scale * g.w.max(g.h) as f32 / l,
        ],
        source,
        numeric,
    )
}

#[cfg(windows)]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let a: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let b: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 1 | 8) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn replace(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

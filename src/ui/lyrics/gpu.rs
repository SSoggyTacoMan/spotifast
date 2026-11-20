//! The lyrics drawn on the graphics card: glyphs filled by Spicy's moving
//! gradient, real Gaussian text shadows for the glow and distance blur, the
//! edge fade, and the Kawarp background. egui has no text shadow and no
//! per-glyph gradient, so both go through one paint callback of our own.
//!
//! Effects follow the reference renderer exactly: an inactive line is only
//! its shadow (transparent text with a white shadow blurred by its distance),
//! an active word is gradient text over a glow shadow. Shadows sharing a blur
//! are drawn into an offscreen buffer together, blurred in two passes, and
//! composited; wide blurs run at a fraction of the resolution.

use std::sync::{Arc, Mutex};

use eframe::egui_glow;
use eframe::glow::{self, HasContext as _};
use egui::emath::TSTransform;
use egui::{Galley, Pos2, Rect, pos2};

/// How a run of glyphs is filled.
#[derive(Clone, Copy, Debug)]
pub enum Fill {
    /// Not drawn: the glyphs only cast their shadow.
    None,
    Solid(f32),
    /// Spicy's karaoke fill: `left` alpha up to `lo`, `right` from `hi`, a
    /// straight blend between. `along` is where the glyphs' origin sits on
    /// the gradient's axis; `vertical` runs it down the line instead.
    Wipe {
        along: f32,
        lo: f32,
        hi: f32,
        left: f32,
        right: f32,
        vertical: bool,
    },
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    /// Gradient coordinate, lo, hi, left alpha, right alpha.
    fill: [f32; 5],
}

const STRIDE: i32 = std::mem::size_of::<Vertex>() as i32;

struct Mesh {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    bounds: Rect,
}

impl Default for Mesh {
    fn default() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            bounds: Rect::NOTHING,
        }
    }
}

impl Mesh {
    fn extend(&mut self, quad: [Vertex; 4]) {
        let base = self.vertices.len() as u32;
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 1, base + 3]);
        for v in quad {
            self.bounds.extend_with(pos2(v.pos[0], v.pos[1]));
        }
        self.vertices.extend_from_slice(&quad);
    }
}

/// Shadow blur radii fall into these buckets (in pixels of sigma), so lines
/// blurred alike share one blur pass.
const SIGMAS: [f32; 14] = [
    0.35, 0.6, 0.85, 1.1, 1.5, 2.0, 2.6, 3.3, 4.2, 5.4, 7.0, 9.0, 12.0, 16.0,
];

fn bucket(sigma_px: f32) -> usize {
    SIGMAS
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - sigma_px).abs().total_cmp(&(b.1 - sigma_px).abs()))
        .map_or(0, |(i, _)| i)
}

/// Everything one stage draws this frame.
pub struct Batch {
    text: Mesh,
    shadows: Vec<Mesh>,
    atlas: [f32; 2],
    ppp: f32,
    /// The lyrics area, for the mask: outer and inner fade distances in
    /// points from its top and bottom.
    mask: Option<(Rect, f32, f32)>,
}

impl Batch {
    pub fn new(ctx: &egui::Context) -> Self {
        let [w, h] = ctx.fonts(|f| f.font_image_size());
        Self {
            text: Mesh {
                bounds: Rect::NOTHING,
                ..Mesh::default()
            },
            shadows: (0..SIGMAS.len())
                .map(|_| Mesh {
                    bounds: Rect::NOTHING,
                    ..Mesh::default()
                })
                .collect(),
            atlas: [w.max(1) as f32, h.max(1) as f32],
            ppp: ctx.pixels_per_point(),
            mask: None,
        }
    }

    /// Fades the lyrics out over `inner` points from the area's top and
    /// bottom, fully gone in the outer `outer`.
    pub fn mask(&mut self, rect: Rect, outer: f32, inner: f32) {
        self.mask = Some((rect, outer, inner));
    }

    /// Adds a galley's glyphs, its top-left at `origin` before `transform`.
    /// `shadow` is (alpha, blur radius in points), drawn under the fill.
    pub fn glyphs(
        &mut self,
        galley: &Galley,
        origin: Pos2,
        transform: TSTransform,
        fill: Fill,
        shadow: Option<(f32, f32)>,
    ) {
        let draw_fill = !matches!(fill, Fill::None | Fill::Solid(0.0));
        let shadow = shadow.filter(|(alpha, _)| *alpha > 0.002);
        if !draw_fill && shadow.is_none() {
            return;
        }
        for placed in &galley.rows {
            let row = &placed.row;
            let mesh = &row.visuals.mesh;
            let range = row.visuals.glyph_vertex_range.clone();
            let mut v = range.start;
            while v + 3 < range.end {
                let quad: [&egui::epaint::Vertex; 4] = [
                    &mesh.vertices[v],
                    &mesh.vertices[v + 1],
                    &mesh.vertices[v + 2],
                    &mesh.vertices[v + 3],
                ];
                v += 4;
                let local = |vertex: &egui::epaint::Vertex| {
                    origin + placed.pos.to_vec2() + vertex.pos.to_vec2()
                };
                let make = |vertex: &egui::epaint::Vertex, fill: Fill| {
                    let at = local(vertex);
                    let screen = transform * at;
                    let g = match fill {
                        Fill::Wipe {
                            along, vertical, ..
                        } => {
                            if vertical {
                                along + (at.y - origin.y)
                            } else {
                                along + (at.x - origin.x)
                            }
                        }
                        _ => 0.0,
                    };
                    let (lo, hi, left, right) = match fill {
                        Fill::None => (0.0, 1.0, 0.0, 0.0),
                        Fill::Solid(a) => (0.0, 1.0, a, a),
                        Fill::Wipe {
                            lo,
                            hi,
                            left,
                            right,
                            ..
                        } => (lo, hi.max(lo + 1e-4), left, right),
                    };
                    Vertex {
                        pos: [screen.x, screen.y],
                        uv: [vertex.uv.x, vertex.uv.y],
                        fill: [g, lo, hi, left, right],
                    }
                };
                if let Some((alpha, radius)) = shadow {
                    let index = bucket(radius / 2.0 * self.ppp);
                    let solid = Fill::Solid(alpha.min(1.0));
                    self.shadows[index].extend(quad.map(|q| make(q, solid)));
                }
                if draw_fill {
                    self.text.extend(quad.map(|q| make(q, fill)));
                }
            }
        }
    }

    /// Hands the batch to the graphics card, painted over `rect`.
    pub fn paint(self, painter: &egui::Painter, rect: Rect) {
        if self.text.vertices.is_empty() && self.shadows.iter().all(|s| s.vertices.is_empty()) {
            return;
        }
        let gpu = gpu(painter.ctx());
        let batch = Mutex::new(Some(self));
        let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
            let Some(batch) = batch.lock().ok().and_then(|mut b| b.take()) else {
                return;
            };
            let mut gpu = gpu.lock().expect("lyrics renderer");
            if let Some(renderer) = gpu.renderer(glow_painter.gl()) {
                renderer.draw(glow_painter, &info, batch);
            }
        });
        painter.add(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        });
    }
}

type Shared = Arc<Mutex<Gpu>>;

#[derive(Default)]
struct Gpu {
    renderer: Option<Option<Renderer>>,
}

impl Gpu {
    fn renderer(&mut self, gl: &Arc<glow::Context>) -> Option<&mut Renderer> {
        self.renderer
            .get_or_insert_with(|| {
                let made = Renderer::new(gl.clone());
                if made.is_none() {
                    log::warn!("lyrics: the graphics card would not build the lyrics shaders");
                }
                made
            })
            .as_mut()
    }

    /// Deletes the renderer while the context that made it is current.
    fn release(&mut self, gl: &glow::Context) {
        if let Some(Some(renderer)) = self.renderer.take() {
            // A window recreated with another context cannot delete these;
            // its old context frees them when it is destroyed.
            if std::ptr::eq(Arc::as_ptr(&renderer.gl), gl) {
                renderer.destroy();
            }
        }
    }
}

/// Frees the lyrics renderer's graphics objects as its window goes away.
/// Without this the context would free them only when it is destroyed, which
/// a window closed to the tray keeps alive.
pub fn release(ctx: &egui::Context, gl: &glow::Context) {
    let shared: Option<Shared> = ctx.data(|data| data.get_temp(egui::Id::new("lyrics-gpu")));
    if let Some(shared) = shared
        && let Ok(mut gpu) = shared.lock()
    {
        gpu.release(gl);
    }
}

fn gpu(ctx: &egui::Context) -> Shared {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::new("lyrics-gpu"), || {
            Arc::new(Mutex::new(Gpu::default()))
        })
        .clone()
    })
}

struct Target {
    framebuffer: glow::Framebuffer,
    texture: glow::Texture,
    size: (i32, i32),
    /// The pixels touched by the last pass (left, bottom, right, top).
    /// Everything outside this area is kept transparent.
    dirty: Option<[i32; 4]>,
}

fn clear_bounds(previous: Option<[i32; 4]>, current: [i32; 4]) -> [i32; 4] {
    previous.map_or(current, |old| {
        [
            old[0].min(current[0]),
            old[1].min(current[1]),
            old[2].max(current[2]),
            old[3].max(current[3]),
        ]
    })
}

/// The area to clear in a blur buffer: the old and new glyph boxes, brought
/// out by the blur's reach. A blur tap reads up to three sigmas beside the
/// box, so clearing only the boxes would let a tap find a word from an
/// earlier frame and draw it again beside the new one.
fn clear_area(
    previous: Option<[i32; 4]>,
    current: [i32; 4],
    reach: i32,
    size: (i32, i32),
) -> [i32; 4] {
    let bounds = clear_bounds(previous, current);
    [
        (bounds[0] - reach).max(0),
        (bounds[1] - reach).max(0),
        (bounds[2] + reach).min(size.0),
        (bounds[3] + reach).min(size.1),
    ]
}

struct Renderer {
    gl: Arc<glow::Context>,
    text: glow::Program,
    blur: glow::Program,
    kawarp: glow::Program,
    blit: glow::Program,
    vao: Option<glow::VertexArray>,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    quad: glow::Buffer,
    targets: Vec<Target>,
    canvas: Option<Target>,
    /// GLES before 3.1 cannot ask a texture its size.
    atlas_size_query: bool,
}

const TEXT_VS: &str = r"
uniform vec2 u_screen;
uniform vec2 u_atlas_size;
IN vec2 a_pos;
IN vec2 a_uv;
IN float a_g;
IN vec4 a_fill;
OUT vec2 v_uv;
OUT float v_g;
OUT vec4 v_fill;
void main() {
    gl_Position = vec4(2.0 * a_pos.x / u_screen.x - 1.0, 1.0 - 2.0 * a_pos.y / u_screen.y, 0.0, 1.0);
    v_uv = a_uv / u_atlas_size;
    v_g = a_g;
    v_fill = a_fill;
}
";

const MASK: &str = r"
uniform vec4 u_mask; // top, bottom (pixels from the framebuffer's bottom), outer, inner
uniform float u_masked;
float mask_at(float y) {
    if (u_masked < 0.5) return 1.0;
    float from_top = u_mask.x - y;
    float from_bottom = y - u_mask.y;
    float d = min(from_top, from_bottom);
    return clamp((d - u_mask.z) / max(u_mask.w - u_mask.z, 0.001), 0.0, 1.0);
}
";

const TEXT_FS: &str = r"
uniform sampler2D u_atlas;
VIN vec2 v_uv;
VIN float v_g;
VIN vec4 v_fill;
void main() {
    float coverage = TEX(u_atlas, v_uv).a;
    float t = clamp((v_g - v_fill.x) / max(v_fill.y - v_fill.x, 0.0001), 0.0, 1.0);
    float alpha = mix(v_fill.z, v_fill.w, t) * coverage * mask_at(gl_FragCoord.y);
    FRAG = vec4(alpha);
}
";

const QUAD_VS: &str = r"
IN vec2 a_pos;
OUT vec2 v_uv;
void main() {
    gl_Position = vec4(a_pos, 0.0, 1.0);
    v_uv = a_pos * 0.5 + 0.5;
}
";

const BLUR_FS: &str = r"
uniform sampler2D u_source;
uniform vec2 u_step;
uniform float u_sigma;
uniform float u_final;
VIN vec2 v_uv;
void main() {
    float sum = 0.0;
    float weights = 0.0;
    int reach = int(ceil(u_sigma * 3.0));
    for (int i = -48; i <= 48; i++) {
        if (i < -reach || i > reach) continue;
        float x = float(i);
        float w = exp(-x * x / (2.0 * u_sigma * u_sigma));
        sum += TEX(u_source, v_uv + u_step * x).a * w;
        weights += w;
    }
    float a = sum / weights;
    if (u_final > 0.5) a *= mask_at(gl_FragCoord.y);
    FRAG = vec4(a);
}
";

const BLIT_FS: &str = r"
uniform sampler2D u_source;
uniform vec4 u_rect; // the area in the source's uv
VIN vec2 v_uv;
void main() {
    FRAG = vec4(TEX(u_source, mix(u_rect.xy, u_rect.zw, v_uv)).rgb, 1.0);
}
";

/// The Kawarp shader, from Spicy Lyrics Mobile's fused AGSL port of
/// @kawarp/core: domain warp, crossfade, vignette, saturation, dither,
/// then the CSS `saturate() brightness()` filter on top.
const KAWARP_FS: &str = r"
uniform sampler2D u_current;
uniform sampler2D u_next;
uniform float u_time;
uniform float u_blend;
uniform float u_scale;
uniform vec2 u_filter; // CSS saturate, brightness
VIN vec2 v_uv;
vec3 mod289v3(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec2 mod289v2(vec2 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec3 permute(vec3 x) { return mod289v3(((x * 34.0) + 1.0) * x); }
float snoise(vec2 v) {
    const vec4 C = vec4(0.211324865405187, 0.366025403784439, -0.577350269189626, 0.024390243902439);
    vec2 i = floor(v + dot(v, C.yy));
    vec2 x0 = v - i + dot(i, C.xx);
    vec2 i1 = (x0.x > x0.y) ? vec2(1.0, 0.0) : vec2(0.0, 1.0);
    vec4 x12 = x0.xyxy + C.xxzz;
    x12.xy -= i1;
    i = mod289v2(i);
    vec3 p = permute(permute(i.y + vec3(0.0, i1.y, 1.0)) + i.x + vec3(0.0, i1.x, 1.0));
    vec3 m = max(0.5 - vec3(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), 0.0);
    m = m * m; m = m * m;
    vec3 x = 2.0 * fract(p * C.www) - 1.0;
    vec3 h = abs(x) - 0.5;
    vec3 ox = floor(x + 0.5);
    vec3 a0 = x - ox;
    m *= 1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h);
    vec3 g;
    g.x = a0.x * x0.x + h.x * x0.y;
    g.y = a0.y * x12.x + h.y * x12.y;
    g.z = a0.z * x12.z + h.z * x12.w;
    return 130.0 * dot(m, g);
}
float hash(vec3 p) {
    p = fract(p * 0.1031);
    p += dot(p, p.zyx + 31.32);
    return fract((p.x + p.y) * p.z);
}
void main() {
    vec2 tc = v_uv;
    vec2 uv = clamp((tc - 0.5) / u_scale + 0.5, 0.0, 1.0);
    float t = u_time * 0.05;
    float centerWeight = 1.0 - smoothstep(0.0, 0.7, length(uv - 0.5));
    float n1 = snoise(uv * 0.35 + vec2(t, t * 0.7));
    float n2 = snoise(uv * 0.35 + vec2(-t * 0.8, t * 0.5) + vec2(50.0, 50.0));
    float n3 = snoise(uv * 0.9 + vec2(t * 1.2, -t) + vec2(100.0, 0.0));
    float n4 = snoise(uv * 0.9 + vec2(-t, t * 1.1) + vec2(0.0, 100.0));
    vec2 warp = vec2(n1 * 0.65 + n3 * 0.35, n2 * 0.65 + n4 * 0.35) * centerWeight;
    vec2 warped = clamp(uv + warp, 0.0, 1.0);
    vec4 color = mix(TEX(u_current, warped), TEX(u_next, warped), u_blend);
    vec2 vc = tc - 0.5;
    color.rgb *= 1.0 - dot(vc, vc) * 0.3;
    float gray = dot(color.rgb, vec3(0.299, 0.587, 0.114));
    color.rgb = mix(vec3(gray), color.rgb, 1.5);
    vec2 grid = tc * vec2(300.0, 150.0) - 0.5;
    vec2 cell = floor(grid);
    vec2 f = grid - cell;
    vec2 lo = clamp(cell, vec2(0.0), vec2(299.0, 149.0));
    vec2 hi = clamp(cell + 1.0, vec2(0.0), vec2(299.0, 149.0));
    float frame = floor(u_time * 60.0);
    float noise = mix(mix(hash(vec3(lo.x, lo.y, frame)), hash(vec3(hi.x, lo.y, frame)), f.x),
                      mix(hash(vec3(lo.x, hi.y, frame)), hash(vec3(hi.x, hi.y, frame)), f.x), f.y);
    color.rgb += vec3((noise - 0.5) * 0.008);
    color.rgb = clamp(color.rgb, 0.0, 1.0);
    float cssGray = dot(color.rgb, vec3(0.213, 0.715, 0.072));
    color.rgb = clamp(mix(vec3(cssGray), color.rgb, u_filter.x), 0.0, 1.0);
    color.rgb *= u_filter.y;
    FRAG = vec4(color.rgb, 1.0);
}
";

fn prelude(gl: &glow::Context, fragment: bool) -> String {
    let version = egui_glow::ShaderVersion::get(gl);
    let mut out = version.version_declaration().to_owned();
    if version.is_embedded() {
        out.push_str("precision highp float;\n");
    }
    if version.is_new_shader_interface() {
        out.push_str("#define IN in\n#define OUT out\n#define VIN in\n#define TEX texture\n");
        if fragment {
            out.push_str("out vec4 f_color;\n#define FRAG f_color\n");
        }
    } else {
        out.push_str("#define IN attribute\n#define OUT varying\n#define VIN varying\n#define TEX texture2D\n#define FRAG gl_FragColor\n");
    }
    out
}

unsafe fn program(
    gl: &glow::Context,
    vs: &str,
    fs: &str,
    attributes: &[&str],
) -> Option<glow::Program> {
    // SAFETY: the context is current inside egui's paint callback.
    unsafe {
        let program = gl.create_program().ok()?;
        let mut shaders = Vec::new();
        let linked = (|| {
            for (kind, source, fragment) in [
                (glow::VERTEX_SHADER, vs, false),
                (glow::FRAGMENT_SHADER, fs, true),
            ] {
                let shader = gl.create_shader(kind).ok()?;
                shaders.push(shader);
                let mut text = prelude(gl, fragment);
                if fragment {
                    text.push_str(MASK);
                }
                text.push_str(source);
                gl.shader_source(shader, &text);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    log::warn!("lyrics shader: {}", gl.get_shader_info_log(shader));
                    return None;
                }
            }
            for shader in &shaders {
                gl.attach_shader(program, *shader);
            }
            for (index, name) in attributes.iter().enumerate() {
                gl.bind_attrib_location(program, index as u32, name);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                log::warn!("lyrics shader: {}", gl.get_program_info_log(program));
                return None;
            }
            Some(())
        })();
        // Attached shaders stay alive with the program; a failed build must
        // leave nothing behind.
        for shader in shaders {
            gl.delete_shader(shader);
        }
        if linked.is_none() {
            gl.delete_program(program);
            return None;
        }
        Some(program)
    }
}

impl Renderer {
    fn new(gl: Arc<glow::Context>) -> Option<Self> {
        // SAFETY: the context is current inside egui's paint callback; every
        // object made here belongs to this renderer.
        unsafe {
            let mut programs = Vec::new();
            for (vs, fs, attributes) in [
                (TEXT_VS, TEXT_FS, &["a_pos", "a_uv", "a_g", "a_fill"][..]),
                (QUAD_VS, BLUR_FS, &["a_pos"][..]),
                (QUAD_VS, KAWARP_FS, &["a_pos"][..]),
                (QUAD_VS, BLIT_FS, &["a_pos"][..]),
            ] {
                match program(&gl, vs, fs, attributes) {
                    Some(program) => programs.push(program),
                    None => {
                        for program in programs {
                            gl.delete_program(program);
                        }
                        return None;
                    }
                }
            }
            let [text, blur, kawarp, blit] = [programs[0], programs[1], programs[2], programs[3]];
            let vao = gl.create_vertex_array().ok();
            let mut buffers = Vec::new();
            for _ in 0..3 {
                let Ok(buffer) = gl.create_buffer() else {
                    if let Some(vao) = vao {
                        gl.delete_vertex_array(vao);
                    }
                    for buffer in buffers {
                        gl.delete_buffer(buffer);
                    }
                    for program in programs {
                        gl.delete_program(program);
                    }
                    return None;
                };
                buffers.push(buffer);
            }
            let [vbo, ebo, quad] = [buffers[0], buffers[1], buffers[2]];
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad));
            let corners: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&corners), glow::STATIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            let atlas_size_query = !egui_glow::ShaderVersion::get(&gl).is_embedded();
            Some(Self {
                gl,
                text,
                blur,
                kawarp,
                blit,
                vao,
                vbo,
                ebo,
                quad,
                targets: Vec::new(),
                canvas: None,
                atlas_size_query,
            })
        }
    }

    /// Deletes everything this renderer made. Called with its context current
    /// as the window goes away; the context would free the objects at once
    /// anyway, but a window closed to the tray keeps the context alive.
    fn destroy(&self) {
        let gl = &self.gl;
        // SAFETY: the caller has the renderer's context current, and these
        // objects are this renderer's alone.
        unsafe {
            gl.delete_program(self.text);
            gl.delete_program(self.blur);
            gl.delete_program(self.kawarp);
            gl.delete_program(self.blit);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
            gl.delete_buffer(self.quad);
            if let Some(vao) = self.vao {
                gl.delete_vertex_array(vao);
            }
            for target in &self.targets {
                gl.delete_texture(target.texture);
                gl.delete_framebuffer(target.framebuffer);
            }
            if let Some(canvas) = &self.canvas {
                gl.delete_texture(canvas.texture);
                gl.delete_framebuffer(canvas.framebuffer);
            }
        }
    }

    unsafe fn target(gl: &glow::Context, size: (i32, i32), filter: u32) -> Option<Target> {
        // SAFETY: as in `new`.
        unsafe {
            let texture = gl.create_texture().ok()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.0,
                size.1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, filter),
                (glow::TEXTURE_MAG_FILTER, filter),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            let framebuffer = match gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(_) => {
                    gl.delete_texture(texture);
                    return None;
                }
            };
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return None;
            }
            // Texture allocation leaves undefined pixels. Every tap outside
            // the current glyph box must see transparent pixels, not a word
            // from an earlier frame or another blur bucket.
            gl.disable(glow::SCISSOR_TEST);
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            Some(Target {
                framebuffer,
                texture,
                size,
                dirty: None,
            })
        }
    }

    /// Two buffers at `1 / scale` of the screen, made or resized as needed.
    unsafe fn targets(&mut self, screen: (i32, i32), scale: i32) -> Option<(usize, usize)> {
        let size = ((screen.0 / scale).max(1), (screen.1 / scale).max(1));
        let first = (scale.trailing_zeros() as usize) * 2;
        while self.targets.len() < first + 2 {
            // SAFETY: as in `new`.
            let made = unsafe { Self::target(&self.gl, size, glow::LINEAR)? };
            self.targets.push(made);
        }
        for i in [first, first + 1] {
            if self.targets[i].size != size {
                // Make the replacement before dropping the old one: a failed
                // allocation must leave the old buffer usable, not a deleted
                // handle that would draw whatever was last in its place.
                // SAFETY: as in `new`.
                let Some(made) = (unsafe { Self::target(&self.gl, size, glow::LINEAR) }) else {
                    log::warn!("lyrics: the graphics card would not make a {size:?} blur buffer");
                    return None;
                };
                let old = std::mem::replace(&mut self.targets[i], made);
                // SAFETY: as in `new`; the old objects are this renderer's.
                unsafe {
                    self.gl.delete_texture(old.texture);
                    self.gl.delete_framebuffer(old.framebuffer);
                }
            }
        }
        Some((first, first + 1))
    }

    unsafe fn bind_quad(&self, program: glow::Program) {
        let gl = &self.gl;
        // SAFETY: as in `new`.
        unsafe {
            gl.use_program(Some(program));
            if let Some(vao) = self.vao {
                gl.bind_vertex_array(Some(vao));
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quad));
            for i in 1..4 {
                gl.disable_vertex_attrib_array(i);
            }
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 8, 0);
        }
    }

    unsafe fn set_mask(
        &self,
        program: glow::Program,
        mask: Option<(Rect, f32, f32)>,
        ppp: f32,
        screen_h: f32,
    ) {
        let gl = &self.gl;
        // SAFETY: as in `new`.
        unsafe {
            gl.use_program(Some(program));
            let masked = gl.get_uniform_location(program, "u_masked");
            let at = gl.get_uniform_location(program, "u_mask");
            match mask {
                Some((rect, outer, inner)) => {
                    gl.uniform_1_f32(masked.as_ref(), 1.0);
                    gl.uniform_4_f32(
                        at.as_ref(),
                        screen_h - rect.top() * ppp,
                        screen_h - rect.bottom() * ppp,
                        outer * ppp,
                        inner * ppp,
                    );
                }
                None => gl.uniform_1_f32(masked.as_ref(), 0.0),
            }
        }
    }

    unsafe fn draw_mesh(
        &self,
        mesh: &Mesh,
        atlas: glow::Texture,
        mesh_atlas: [f32; 2],
        screen_points: [f32; 2],
    ) {
        let gl = &self.gl;
        // SAFETY: as in `new`.
        unsafe {
            gl.use_program(Some(self.text));
            if let Some(vao) = self.vao {
                gl.bind_vertex_array(Some(vao));
            }
            gl.uniform_2_f32(
                gl.get_uniform_location(self.text, "u_screen").as_ref(),
                screen_points[0],
                screen_points[1],
            );
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(atlas));
            let (w, h) = if self.atlas_size_query {
                (
                    gl.get_tex_level_parameter_i32(glow::TEXTURE_2D, 0, glow::TEXTURE_WIDTH),
                    gl.get_tex_level_parameter_i32(glow::TEXTURE_2D, 0, glow::TEXTURE_HEIGHT),
                )
            } else {
                (0, 0)
            };
            let (w, h) = if w > 0 && h > 0 {
                (w as f32, h as f32)
            } else {
                (mesh_atlas[0], mesh_atlas[1])
            };
            gl.uniform_2_f32(
                gl.get_uniform_location(self.text, "u_atlas_size").as_ref(),
                w,
                h,
            );
            gl.uniform_1_i32(gl.get_uniform_location(self.text, "u_atlas").as_ref(), 0);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytes_of(&mesh.vertices),
                glow::STREAM_DRAW,
            );
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytes_of(&mesh.indices),
                glow::STREAM_DRAW,
            );
            for i in 0..4 {
                gl.enable_vertex_attrib_array(i);
            }
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, STRIDE, 0);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, STRIDE, 8);
            gl.vertex_attrib_pointer_f32(2, 1, glow::FLOAT, false, STRIDE, 16);
            gl.vertex_attrib_pointer_f32(3, 4, glow::FLOAT, false, STRIDE, 20);
            gl.draw_elements(
                glow::TRIANGLES,
                mesh.indices.len() as i32,
                glow::UNSIGNED_INT,
                0,
            );
        }
    }

    fn draw(&mut self, painter: &egui_glow::Painter, info: &egui::PaintCallbackInfo, batch: Batch) {
        let Some(atlas) = painter.texture(egui::TextureId::default()) else {
            return;
        };
        let [sw, sh] = info.screen_size_px;
        let screen = (sw as i32, sh as i32);
        let ppp = info.pixels_per_point;
        let screen_points = [sw as f32 / ppp, sh as f32 / ppp];
        let clip = info.clip_rect_in_pixels();
        let gl = self.gl.clone();
        // SAFETY: egui calls this with its context current and restores its
        // own state after; the framebuffer and viewport are put back here.
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.enable(glow::BLEND);
            gl.blend_equation(glow::FUNC_ADD);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            for (index, mesh) in batch.shadows.iter().enumerate() {
                if mesh.vertices.is_empty() {
                    continue;
                }
                let sigma_px = SIGMAS[index];
                let scale = if sigma_px < 2.5 {
                    1
                } else if sigma_px < 6.0 {
                    2
                } else {
                    4
                };
                let Some((a, b)) = self.targets(screen, scale) else {
                    continue;
                };
                let sigma = sigma_px / scale as f32;
                let size = self.targets[a].size;
                // The glyphs' box and the blur's reach, in this buffer.
                let reach = sigma_px * 3.0 + 2.0;
                let clear_reach = (sigma * 3.0).ceil() as i32 + 2;
                let bounds = mesh.bounds;
                let x0 = ((bounds.left() * ppp - reach) / scale as f32)
                    .floor()
                    .max(0.0) as i32;
                let x1 = ((bounds.right() * ppp + reach) / scale as f32)
                    .ceil()
                    .min(size.0 as f32) as i32;
                let y0 = ((sh as f32 - bounds.bottom() * ppp - reach) / scale as f32)
                    .floor()
                    .max(0.0) as i32;
                let y1 = ((sh as f32 - bounds.top() * ppp + reach) / scale as f32)
                    .ceil()
                    .min(size.1 as f32) as i32;
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                gl.enable(glow::SCISSOR_TEST);
                gl.viewport(0, 0, size.0, size.1);
                for index in [a, b] {
                    let target = &mut self.targets[index];
                    let current = [x0, y0, x1, y1];
                    let clear = clear_area(target.dirty, current, clear_reach, size);
                    target.dirty = Some(current);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(target.framebuffer));
                    // Clear the old damage as well as the new box, out to
                    // where this buffer's blur reads. Clearing only the
                    // boxes leaves ghost words that later blur passes
                    // sample, especially when scrolling or seeking.
                    gl.scissor(clear[0], clear[1], clear[2] - clear[0], clear[3] - clear[1]);
                    gl.clear_color(0.0, 0.0, 0.0, 0.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
                gl.scissor(x0, y0, x1 - x0, y1 - y0);
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.targets[a].framebuffer));
                self.set_mask(self.text, None, ppp, sh as f32);
                self.draw_mesh(mesh, atlas, batch.atlas, screen_points);
                // Across, into the second buffer.
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(self.targets[b].framebuffer));
                self.bind_quad(self.blur);
                gl.disable(glow::BLEND);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[a].texture));
                gl.uniform_1_i32(gl.get_uniform_location(self.blur, "u_source").as_ref(), 0);
                gl.uniform_2_f32(
                    gl.get_uniform_location(self.blur, "u_step").as_ref(),
                    1.0 / size.0 as f32,
                    0.0,
                );
                gl.uniform_1_f32(
                    gl.get_uniform_location(self.blur, "u_sigma").as_ref(),
                    sigma.max(0.01),
                );
                gl.uniform_1_f32(gl.get_uniform_location(self.blur, "u_final").as_ref(), 0.0);
                self.set_mask(self.blur, None, ppp, sh as f32);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
                // Down, onto the screen.
                gl.enable(glow::BLEND);
                gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
                gl.viewport(0, 0, screen.0, screen.1);
                gl.scissor(
                    clip.left_px,
                    clip.from_bottom_px,
                    clip.width_px,
                    clip.height_px,
                );
                gl.bind_texture(glow::TEXTURE_2D, Some(self.targets[b].texture));
                gl.uniform_2_f32(
                    gl.get_uniform_location(self.blur, "u_step").as_ref(),
                    0.0,
                    1.0 / size.1 as f32,
                );
                gl.uniform_1_f32(gl.get_uniform_location(self.blur, "u_final").as_ref(), 1.0);
                self.set_mask(self.blur, batch.mask, ppp, sh as f32);
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
            gl.viewport(0, 0, screen.0, screen.1);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            if !batch.text.vertices.is_empty() {
                gl.use_program(Some(self.text));
                self.set_mask(self.text, batch.mask, ppp, sh as f32);
                self.draw_mesh(&batch.text, atlas, batch.atlas, screen_points);
            }
            for i in 0..4 {
                gl.disable_vertex_attrib_array(i);
            }
            if self.vao.is_some() {
                gl.bind_vertex_array(None);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_background(
        &mut self,
        painter: &egui_glow::Painter,
        info: &egui::PaintCallbackInfo,
        current: glow::Texture,
        next: glow::Texture,
        time: f32,
        blend: f32,
        scale: f32,
        filter: (f32, f32),
    ) {
        let gl = self.gl.clone();
        // SAFETY: as in `draw`.
        unsafe {
            if self.canvas.is_none() {
                self.canvas = Self::target(&gl, (300, 150), glow::LINEAR);
            }
            let Some(canvas) = self.canvas.as_ref() else {
                return;
            };
            let (framebuffer, texture) = (canvas.framebuffer, canvas.texture);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.viewport(0, 0, 300, 150);
            self.bind_quad(self.kawarp);
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(current));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(next));
            let program = self.kawarp;
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_current").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_next").as_ref(), 1);
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_time").as_ref(), time);
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_blend").as_ref(), blend);
            gl.uniform_1_f32(gl.get_uniform_location(program, "u_scale").as_ref(), scale);
            gl.uniform_2_f32(
                gl.get_uniform_location(program, "u_filter").as_ref(),
                filter.0,
                filter.1,
            );
            self.set_mask(program, None, 1.0, 1.0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.active_texture(glow::TEXTURE0);
            // Stretched over the area, the browser's bilinear upscale.
            gl.bind_framebuffer(glow::FRAMEBUFFER, painter.intermediate_fbo());
            let viewport = info.viewport_in_pixels();
            gl.viewport(
                viewport.left_px,
                viewport.from_bottom_px,
                viewport.width_px,
                viewport.height_px,
            );
            let clip = info.clip_rect_in_pixels();
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(
                clip.left_px,
                clip.from_bottom_px,
                clip.width_px,
                clip.height_px,
            );
            self.bind_quad(self.blit);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.uniform_1_i32(gl.get_uniform_location(self.blit, "u_source").as_ref(), 0);
            gl.uniform_4_f32(
                gl.get_uniform_location(self.blit, "u_rect").as_ref(),
                0.0,
                0.0,
                1.0,
                1.0,
            );
            self.set_mask(self.blit, None, 1.0, 1.0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.disable_vertex_attrib_array(0);
            if self.vao.is_some() {
                gl.bind_vertex_array(None);
            }
        }
    }
}

/// One frame of the Kawarp background over `rect`.
#[allow(clippy::too_many_arguments)]
pub fn background(
    painter: &egui::Painter,
    rect: Rect,
    current: egui::TextureId,
    next: egui::TextureId,
    time: f32,
    blend: f32,
    scale: f32,
    filter: (f32, f32),
) {
    let gpu = gpu(painter.ctx());
    let callback = egui_glow::CallbackFn::new(move |info, glow_painter| {
        let (Some(current), Some(next)) =
            (glow_painter.texture(current), glow_painter.texture(next))
        else {
            return;
        };
        let mut gpu = gpu.lock().expect("lyrics renderer");
        if let Some(renderer) = gpu.renderer(glow_painter.gl()) {
            renderer.draw_background(
                glow_painter,
                &info,
                current,
                next,
                time,
                blend,
                scale,
                filter,
            );
        }
    });
    painter.add(egui::PaintCallback {
        rect,
        callback: Arc::new(callback),
    });
}

fn bytes_of<T: Copy>(items: &[T]) -> &[u8] {
    // SAFETY: `T` is plain old data here (f32 arrays, u32 and `Vertex`, which is
    // `repr(C)` floats), so its bytes are initialised and any view of them is valid.
    unsafe { std::slice::from_raw_parts(items.as_ptr().cast(), std::mem::size_of_val(items)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_buckets_cover_every_radius() {
        assert_eq!(bucket(0.0), 0);
        assert_eq!(bucket(100.0), SIGMAS.len() - 1);
        assert_eq!(bucket(2.0), 5);
    }

    #[test]
    fn a_blur_pass_clears_past_every_tap() {
        let old = [10, 20, 80, 100];
        let moved = [40, 120, 90, 180];
        assert_eq!(clear_area(None, moved, 6, (1000, 1000)), [34, 114, 96, 186]);
        assert_eq!(
            clear_area(Some(old), moved, 6, (1000, 1000)),
            [4, 14, 96, 186]
        );
        assert_eq!(
            clear_area(Some([0, 0, 1000, 1000]), [0, 0, 1, 1], 6, (1000, 1000)),
            [0, 0, 1000, 1000],
            "brought out, then kept inside the buffer"
        );
    }
}

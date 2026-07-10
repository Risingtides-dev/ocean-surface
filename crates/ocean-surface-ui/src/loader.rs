//! Ocean loader components — WebGL-based thinking/loading indicators.
//! Ported from mockups/loader-engines/ (soundings-thinking.js, glkit.js).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::{
    CanvasRenderingContext2d, HtmlCanvasElement, WebGlBuffer, WebGlProgram,
    WebGlRenderingContext, WebGlShader, WebGlTexture, WebGlUniformLocation, WebglLoseContext,
};

static UID: AtomicUsize = AtomicUsize::new(0);

// ── Shader sources ─────────────────────────────────────────────────────

/// Fullscreen triangle vertex shader — feeds UVs to the fragment stage.
const QUAD_VS: &str =
    "attribute vec2 aP; varying vec2 vUv; void main(){ vUv = aP*0.5+0.5; gl_Position = vec4(aP,0.,1.); }";

/// Fullscreen triangle vertex data (one triangle covers clip space).
const QUAD_DATA: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];

/// Soundings "thinking…" fragment shader — ported verbatim from
/// mockups/loader-engines/soundings-thinking.js, with the six color
/// constants promoted to uniforms (fed from tokens.css custom properties).
const THINKING_FS: &str = r##"precision highp float;
varying vec2 vUv;
uniform vec2 uRes;
uniform float uT;
uniform float uPing;
uniform vec2 uSrcP;
uniform sampler2D uWord;
uniform float uWordAspect;
uniform vec3 uIndigo;
uniform vec3 uCy;
uniform vec3 uHot;
uniform vec3 uFoam;
uniform vec3 uDim;
uniform vec3 uBackground;

float hash21(vec2 q){ return fract(sin(dot(q, vec2(127.1, 311.7))) * 43758.5453); }

float field(vec2 q, float t){
  float h = 0.0;
  h += 0.30 * sin(dot(q, vec2(4.0, 2.6)) + t * 0.6);
  h += 0.22 * sin(dot(q, vec2(-3.1, 3.8)) + t * 0.45 + 1.7);
  float te = t - uPing;
  if (te > 0.0) {
    float r = distance(q, uSrcP);
    float env = exp(-pow(r - 0.85 * te, 2.0) * 22.0) * exp(-te * 1.1);
    h += 0.9 * sin(r * 26.0 - te * 11.0) * env;
  }
  return h;
}

void main(){
  float aspect = uRes.x / uRes.y;
  vec2 p = (vUv - 0.5) * vec2(aspect, 1.0);
  float te = uT - uPing;

  float h = field(p, uT);
  vec3 col = mix(uBackground * 0.96, uBackground * 1.13 + vec3(0.0, 0.0, 0.004), vUv.y);
  col = mix(col, uIndigo * 0.22, smoothstep(-0.3, -1.0, h) * 0.5);

  float f = h * 2.1;
  float g = 1.0 - abs(fract(f) - 0.5) * 2.0;
  float line = pow(g, 16.0);
  float amp = clamp(abs(h) * 1.2, 0.0, 1.0);
  vec3 ink = mix(mix(uIndigo * 0.6, uIndigo * 0.9, amp), mix(uCy * 0.55, uCy, amp), step(0.0, h));
  col += ink * line * (0.15 + 0.28 * amp);

  float breathe = 0.5 + 0.5 * sin(uT * 2.4);
  vec2 dd = p - uSrcP;
  float flash = te > 0.0 ? exp(-te * 5.0) : 0.0;
  col += uHot * (0.22 + 0.32 * breathe + 0.85 * flash) * exp(-dot(dd, dd) / 0.0010);
  col += uCy * 0.20 * exp(-dot(dd, dd) / 0.008);

  float hw = 0.30 * aspect;
  float hh = hw / uWordAspect;
  vec2 wp = vec2((p.x - 0.16) / (2.0 * hw) + 0.5, p.y / (2.0 * hh) + 0.5);
  float inW = step(abs(wp.x - 0.5), 0.5) * step(abs(wp.y - 0.5), 0.5);
  vec4 s = texture2D(uWord, clamp(wp, 0.0, 1.0));
  float r = distance(p, uSrcP);
  float front = te > 0.0 ? 0.85 * te - r : -1.0;
  float sweep = exp(-front * front * 120.0);
  vec3 tcol = mix(uDim, uCy, 0.20) * (0.92 + 0.08 * breathe);
  tcol = mix(tcol, uCy * 1.1, sweep * 0.7);
  tcol = mix(tcol, uFoam, sweep * 0.5);
  col += tcol * s.a * inW;
  vec4 sr = texture2D(uWord, clamp(wp + vec2(0.006 * sweep, 0.0), 0.0, 1.0));
  vec4 sb = texture2D(uWord, clamp(wp - vec2(0.006 * sweep, 0.0), 0.0, 1.0));
  col += (vec3(1.0, 0.35, 0.45) * sr.a + vec3(0.35, 0.55, 1.0) * sb.a) * sweep * 0.10 * inW;

  col += (hash21(gl_FragCoord.xy + fract(uT) * 7.0) - 0.5) * 0.008;
  gl_FragColor = vec4(col, 1.0);
}
"##;

/// Soundings hero landing fragment shader — ported verbatim from
/// mockups/loader-engines/soundings2.js (hero mode, chrome=false).
/// Color constants promoted to uniforms fed from tokens.css custom properties.
const HERO_FS: &str = r##"precision highp float;
varying vec2 vUv;
uniform vec2 uRes;
uniform float uT;
uniform float uEn;
uniform vec4 uEvt[10];
uniform vec4 uSrc;
uniform float uKick;
uniform float uLineScale;
uniform float uLineExp;
uniform float uChroma;
uniform float uGain;
uniform float uQuiet;
uniform float uC;
uniform float uDecay;
uniform vec3 uIndigo;
uniform vec3 uCy;
uniform vec3 uHot;

const vec3 ABYSS  = vec3(0.010, 0.014, 0.034);

float hash21(vec2 q){ return fract(sin(dot(q, vec2(127.1, 311.7))) * 43758.5453); }

float field(vec2 q, float t){
  float h = 0.0;
  h += 0.24 * sin(dot(q, vec2(3.1, 1.8)) + t * 0.50 * uEn) * uQuiet;
  h += 0.19 * sin(dot(q, vec2(-2.4, 2.9)) + t * 0.37 * uEn + 1.7) * uQuiet;
  h += 0.06 * sin(dot(q, vec2(6.0, -4.2)) + t * 0.90 * uEn + 0.6) * uQuiet;
  for (int i = 0; i < 10; i++) {
    vec4 E = uEvt[i];
    float te = t - E.z;
    float live = step(0.0, te) * step(0.001, E.w);
    float r = distance(q, E.xy);
    float cf = uC;
    float cs = uC * 0.58;
    float envF = exp(-pow(r - cf * te, 2.0) * 16.0) * exp(-te * 0.80 * uDecay);
    float envS = exp(-pow(r - cs * te, 2.0) * 9.0) * exp(-te * 0.45 * uDecay);
    float res = exp(-r * 4.5) * exp(-te * 0.55 * uDecay) * sin(r * 30.0 - te * 5.5 * uC) * 0.35;
    h += live * E.w * (sin(r * 22.0 - te * 14.5 * cf) * envF
                     + 0.55 * sin(r * 12.0 - te * 14.4 * cs) * envS
                     + res);
  }
  return h;
}

float lines(float h){
  float f = h * uLineScale;
  float g = 1.0 - abs(fract(f) - 0.5) * 2.0;
  return pow(g, uLineExp);
}

void main(){
  float aspect = uRes.x / uRes.y;
  vec2 p = (vUv - 0.5) * vec2(aspect, 1.0);
  vec2 q = vec2(p.x * (1.0 + p.y * 0.45), p.y * 1.35);

  float h = field(q, uT);
  float hr = field(q + vec2(0.006 * uChroma, 0.0), uT);
  float hb = field(q - vec2(0.006 * uChroma, 0.0), uT);
  float hu = field(q + vec2(0.0, 0.006), uT);

  vec2 grad = vec2(hr - h, hu - h) * 90.0;
  vec3 col = ABYSS * (0.72 + 0.42 * clamp(vUv.y + grad.y * 0.05, 0.0, 1.0));
  col += uIndigo * 0.05 * clamp(-grad.y, 0.0, 1.5);
  col = mix(col, uIndigo * 0.32, smoothstep(-0.30, -1.05, h) * 0.65);
  col *= 1.0 - 0.5 * dot(p, p);

  float amp = clamp(abs(h) * 1.3, 0.0, 1.0);
  vec3 inkPos = mix(uCy * 0.6, uCy, amp);
  inkPos = mix(inkPos, uHot, smoothstep(0.72, 1.12, h));
  vec3 inkNeg = mix(uIndigo * 0.55, uIndigo * 0.95, amp);
  vec3 ink = mix(inkNeg, inkPos, step(0.0, h));
  col += ink * lines(h) * (0.28 + 0.55 * amp) * uGain;
  col += vec3(1.0, 0.35, 0.45) * lines(hr) * 0.040 * uChroma;
  col += vec3(0.35, 0.55, 1.0) * lines(hb) * 0.040 * uChroma;

  col += uCy * pow(max(h, 0.0) * 0.62, 4.0) * 0.30;

  vec2 dd = p - uSrc.xy;
  col += uHot * uSrc.z * 0.8 * exp(-dot(dd, dd) / (uSrc.w * uSrc.w));
  col += uCy * uSrc.z * 0.45 * exp(-dot(dd, dd) / (uSrc.w * uSrc.w * 8.0));

  col *= 1.0 + uKick * 0.32;

  col += (hash21(gl_FragCoord.xy + fract(uT) * 7.0) - 0.5) * 0.012;
  gl_FragColor = vec4(col, 1.0);
}
"##;

/// Letter vertex shader — one quad per letter, positioned with uPos/uScl.
const LETTER_VS: &str =
    "attribute vec2 aP; uniform vec2 uPos; uniform vec2 uScl; varying vec2 vUv; void main(){ vUv = aP * 0.5 + 0.5; gl_Position = vec4(uPos + aP * uScl, 0.0, 1.0); }";

/// Letter fragment shader — per-letter texture with flash/fringe/split.
const LETTER_FS: &str = r##"precision mediump float;
varying vec2 vUv;
uniform sampler2D uTex;
uniform float uLit;
uniform float uFlash;
uniform float uSplit;
void main(){
  vec4 s = texture2D(uTex, vUv);
  vec3 base = mix(s.rgb, vec3(0.918, 0.988, 1.0), uFlash);
  float aR = texture2D(uTex, vUv + vec2(uSplit, 0.0)).a;
  float aB = texture2D(uTex, vUv - vec2(uSplit, 0.0)).a;
  vec3 fringe = vec3(1.0, 0.35, 0.45) * aR + vec3(0.35, 0.55, 1.0) * aB;
  float a = s.a * (0.03 + 0.97 * uLit);
  vec3 col = base + fringe * uSplit * 26.0;
  gl_FragColor = vec4(col, min(a + uFlash * s.a * 0.4, 1.0));
}
"##;

// ── Color helpers ──────────────────────────────────────────────────────

/// Parse a hex color string (with or without `#`) into [r, g, b] in 0..1.
fn parse_hex(hex: &str) -> [f32; 3] {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return [0.0, 0.0, 0.0];
    }
    let ch = |i: usize| -> f32 {
        u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0) as f32 / 255.0
    };
    [ch(0), ch(2), ch(4)]
}

/// Read a CSS custom property from `:root` and convert its computed value
/// to an [r, g, b] triplet in 0..1.  Handles both six-digit hex and
/// `rgb(r, g, b)` / `rgba(r, g, b, a)` computed forms.
fn read_token_vec3(name: &str) -> [f32; 3] {
    let val = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| {
            web_sys::window()
                .and_then(|w| w.get_computed_style(&el).ok())
                .flatten()
        })
        .and_then(|style| style.get_property_value(name).ok())
        .unwrap_or_default()
        .trim()
        .to_string();

    // The computed value is typically the authored hex string or an rgb() form.
    if val.starts_with('#') {
        return parse_hex(&val);
    }
    if val.starts_with("rgb") {
        let start = val.find('(').unwrap_or(0) + 1;
        let end = val.find(')').unwrap_or(val.len());
        let inner = &val[start..end];
        let parts: Vec<f32> = inner
            .split(',')
            .filter_map(|p| p.trim().parse::<f32>().ok())
            .collect();
        if parts.len() >= 3 {
            return [parts[0] / 255.0, parts[1] / 255.0, parts[2] / 255.0];
        }
    }
    [0.0, 0.0, 0.0]
}

// ── GL helpers (glkit.js port) ────────────────────────────────────────

fn compile_shader(gl: &WebGlRenderingContext, kind: u32, src: &str) -> Result<WebGlShader, String> {
    let s = gl.create_shader(kind).ok_or("createShader")?;
    gl.shader_source(&s, src);
    gl.compile_shader(&s);
    if gl
        .get_shader_parameter(&s, WebGlRenderingContext::COMPILE_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(s)
    } else {
        let log = gl
            .get_shader_info_log(&s)
            .unwrap_or_else(|| "unknown shader error".into());
        gl.delete_shader(Some(&s));
        Err(log)
    }
}

fn link_program(
    gl: &WebGlRenderingContext,
    vs: &WebGlShader,
    fs: &WebGlShader,
) -> Result<WebGlProgram, String> {
    let p = gl.create_program().ok_or("createProgram")?;
    gl.attach_shader(&p, vs);
    gl.attach_shader(&p, fs);
    gl.link_program(&p);
    if gl
        .get_program_parameter(&p, WebGlRenderingContext::LINK_STATUS)
        .as_bool()
        .unwrap_or(false)
    {
        Ok(p)
    } else {
        let log = gl
            .get_program_info_log(&p)
            .unwrap_or_else(|| "unknown link error".into());
        gl.delete_program(Some(&p));
        Err(log)
    }
}

fn build_uniform_map(
    gl: &WebGlRenderingContext,
    prog: &WebGlProgram,
) -> HashMap<String, WebGlUniformLocation> {
    let mut map = HashMap::new();
    let n = gl
        .get_program_parameter(prog, WebGlRenderingContext::ACTIVE_UNIFORMS)
        .as_f64()
        .unwrap_or(0.0) as u32;
    for i in 0..n {
        if let Some(info) = gl.get_active_uniform(prog, i) {
            let name = info.name().replace("[0]", "");
            if let Some(loc) = gl.get_uniform_location(prog, &name) {
                map.insert(name, loc);
            }
        }
    }
    map
}

fn create_fullscreen_quad(gl: &WebGlRenderingContext) -> WebGlBuffer {
    let buf = gl.create_buffer().expect("createBuffer");
    gl.bind_buffer(WebGlRenderingContext::ARRAY_BUFFER, Some(&buf));
    unsafe {
        let data = js_sys::Float32Array::view(&QUAD_DATA);
        gl.buffer_data_with_array_buffer_view(
            WebGlRenderingContext::ARRAY_BUFFER,
            &data,
            WebGlRenderingContext::STATIC_DRAW,
        );
    }
    buf
}

fn draw_quad(gl: &WebGlRenderingContext, buf: &WebGlBuffer, prog: &WebGlProgram) {
    gl.bind_buffer(WebGlRenderingContext::ARRAY_BUFFER, Some(buf));
    let loc = gl.get_attrib_location(prog, "aP") as u32;
    gl.enable_vertex_attrib_array(loc);
    gl.vertex_attrib_pointer_with_i32(loc, 2, WebGlRenderingContext::FLOAT, false, 0, 0);
    gl.draw_arrays(WebGlRenderingContext::TRIANGLES, 0, 3);
}

/// Upload an `HtmlCanvasElement` to a new GL texture (RGBA, LINEAR,
/// CLAMP_TO_EDGE).  The caller owns the returned texture and must delete it.
fn make_texture(gl: &WebGlRenderingContext, source: &HtmlCanvasElement) -> WebGlTexture {
    let tex = gl.create_texture().expect("createTexture");
    gl.bind_texture(WebGlRenderingContext::TEXTURE_2D, Some(&tex));
    gl.pixel_storei(WebGlRenderingContext::UNPACK_FLIP_Y_WEBGL, 1);
    gl.tex_image_2d_with_u32_and_u32_and_canvas(
        WebGlRenderingContext::TEXTURE_2D,
        0,
        WebGlRenderingContext::RGBA as i32,
        WebGlRenderingContext::RGBA,
        WebGlRenderingContext::UNSIGNED_BYTE,
        source,
    )
    .expect("texImage2D canvas");
    gl.pixel_storei(WebGlRenderingContext::UNPACK_FLIP_Y_WEBGL, 0);
    gl.tex_parameteri(
        WebGlRenderingContext::TEXTURE_2D,
        WebGlRenderingContext::TEXTURE_MIN_FILTER,
        WebGlRenderingContext::LINEAR as i32,
    );
    gl.tex_parameteri(
        WebGlRenderingContext::TEXTURE_2D,
        WebGlRenderingContext::TEXTURE_MAG_FILTER,
        WebGlRenderingContext::LINEAR as i32,
    );
    gl.tex_parameteri(
        WebGlRenderingContext::TEXTURE_2D,
        WebGlRenderingContext::TEXTURE_WRAP_S,
        WebGlRenderingContext::CLAMP_TO_EDGE as i32,
    );
    gl.tex_parameteri(
        WebGlRenderingContext::TEXTURE_2D,
        WebGlRenderingContext::TEXTURE_WRAP_T,
        WebGlRenderingContext::CLAMP_TO_EDGE as i32,
    );
    tex
}

/// Initialise a WebGL context on `canvas` (alpha:false, antialias:true,
/// DPR ≤ 2, viewport sized to CSS pixels × DPR, context-lost prevention).
fn init_gl(canvas: &HtmlCanvasElement) -> Result<WebGlRenderingContext, String> {
    let opts = js_sys::Object::new();
    js_sys::Reflect::set(&opts, &JsValue::from_str("alpha"), &JsValue::FALSE)
        .map_err(|_| "opts.alpha")?;
    js_sys::Reflect::set(&opts, &JsValue::from_str("antialias"), &JsValue::TRUE)
        .map_err(|_| "opts.antialias")?;
    js_sys::Reflect::set(
        &opts,
        &JsValue::from_str("premultipliedAlpha"),
        &JsValue::FALSE,
    )
    .map_err(|_| "opts.premultipliedAlpha")?;
    js_sys::Reflect::set(
        &opts,
        &JsValue::from_str("preserveDrawingBuffer"),
        &JsValue::TRUE,
    )
    .map_err(|_| "opts.preserveDrawingBuffer")?;

    let gl: WebGlRenderingContext = canvas
        .get_context_with_context_options("webgl", &opts)
        .map_err(|_| "getContext webgl")?
        .ok_or("WebGL unavailable")?
        .dyn_into()
        .map_err(|_| "WebGL cast")?;

    // Prevent context-lost from killing the page; engine checks a flag.
    let lost_flag = Rc::new(std::cell::Cell::new(false));
    let lost_cb = Closure::wrap(Box::new(move |e: web_sys::WebGlContextEvent| {
        e.prevent_default();
        lost_flag.set(true);
    }) as Box<dyn FnMut(_)>);
    let _ = canvas
        .add_event_listener_with_callback("webglcontextlost", lost_cb.as_ref().unchecked_ref());
    lost_cb.forget();

    let dpr = web_sys::window()
        .map(|w| w.device_pixel_ratio().min(2.0))
        .unwrap_or(1.0);
    let w = (canvas.client_width() as f64 * dpr).round() as u32;
    let h = (canvas.client_height() as f64 * dpr).round() as u32;
    let w = u32::max(w, 2);
    let h = u32::max(h, 2);
    canvas.set_width(w);
    canvas.set_height(h);
    gl.viewport(0, 0, w as i32, h as i32);

    Ok(gl)
}

// ── "thinking…" texture rasterisation ──────────────────────────────────

struct ThinkingWord {
    tex: WebGlTexture,
    aspect: f32,
}

fn make_thinking_texture(gl: &WebGlRenderingContext) -> Result<ThinkingWord, String> {
    // Resolve font family from the --wm-font token on :root.
    let font_family = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| {
            web_sys::window()
                .and_then(|w| w.get_computed_style(&el).ok())
                .flatten()
        })
        .and_then(|style| style.get_property_value("--wm-font").ok())
        .unwrap_or_else(|| "'Poppins', system-ui, sans-serif".into());

    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .map_err(|_| "createElement")?
        .dyn_into()
        .map_err(|_| "HtmlCanvasElement cast")?;
    let ctx: CanvasRenderingContext2d = canvas
        .get_context("2d")
        .map_err(|_| "getContext 2d")?
        .ok_or("no 2d context")?
        .dyn_into()
        .map_err(|_| "CanvasRenderingContext2d cast")?;

    let font = format!("600 96px {}", font_family);
    ctx.set_font(&font);
    let metrics = ctx
        .measure_text("thinking\u{2026}")
        .map_err(|_| "measureText")?;
    let w = (metrics.width() + 40.0).ceil() as u32;
    let h: u32 = 140;
    canvas.set_width(w);
    canvas.set_height(h);

    ctx.set_font(&font);
    ctx.set_text_baseline("middle");
    ctx.set_fill_style_str("white");
    ctx.fill_text("thinking\u{2026}", 20.0, h as f64 / 2.0 + 6.0)
        .map_err(|_| "fillText")?;

    let tex = make_texture(gl, &canvas);
    Ok(ThinkingWord {
        tex,
        aspect: w as f32 / h as f32,
    })
}

// ── Engine — bundles GL state, token colors, and animation bookkeeping ──

struct Engine {
    gl: WebGlRenderingContext,
    prog: WebGlProgram,
    vs: WebGlShader,
    fs: WebGlShader,
    quad_buf: WebGlBuffer,
    word_tex: WebGlTexture,
    word_aspect: f32,
    uniforms: HashMap<String, WebGlUniformLocation>,
    canvas: HtmlCanvasElement,
    lose_ext: Option<WebglLoseContext>,
    // Token colors (pre-resolved from CSS custom properties, 0..1).
    tok_indigo: [f32; 3],
    tok_cy: [f32; 3],
    tok_hot: [f32; 3],
    tok_foam: [f32; 3],
    tok_dim: [f32; 3],
    tok_bg: [f32; 3],
    // Ping state (drives the emitter wavefront).
    ping: f64,
    next_ping: f64,
}

impl Engine {
    fn render(&mut self, t: f64) {
        let gl = &self.gl;
        let w = self.canvas.width();
        let h = self.canvas.height();

        if t >= self.next_ping {
            self.ping = t;
            self.next_ping = t + 1.9 + js_sys::Math::random() * 0.8;
        }

        // Source drifts gently on y.
        let src_y = 0.045 * (t * 1.05).sin();
        const SRC_X: f32 = -1.15;

        gl.viewport(0, 0, w as i32, h as i32);
        gl.disable(WebGlRenderingContext::BLEND);
        gl.use_program(Some(&self.prog));

        let u = &self.uniforms;
        macro_rules! u1f {
            ($name:expr, $val:expr) => {
                if let Some(loc) = u.get($name) {
                    gl.uniform1f(Some(loc), $val);
                }
            };
        }
        macro_rules! u2f {
            ($name:expr, $x:expr, $y:expr) => {
                if let Some(loc) = u.get($name) {
                    gl.uniform2f(Some(loc), $x, $y);
                }
            };
        }
        macro_rules! u3fv {
            ($name:expr, $v:expr) => {
                if let Some(loc) = u.get($name) {
                    gl.uniform3fv_with_f32_array(Some(loc), &$v);
                }
            };
        }
        macro_rules! u1i {
            ($name:expr, $val:expr) => {
                if let Some(loc) = u.get($name) {
                    gl.uniform1i(Some(loc), $val);
                }
            };
        }

        u2f!("uRes", w as f32, h as f32);
        u1f!("uT", t as f32);
        u1f!("uPing", self.ping as f32);
        u2f!("uSrcP", SRC_X, src_y as f32);
        u3fv!("uIndigo", self.tok_indigo);
        u3fv!("uCy", self.tok_cy);
        u3fv!("uHot", self.tok_hot);
        u3fv!("uFoam", self.tok_foam);
        u3fv!("uDim", self.tok_dim);
        u3fv!("uBackground", self.tok_bg);
        u1f!("uWordAspect", self.word_aspect);

        gl.active_texture(WebGlRenderingContext::TEXTURE0);
        gl.bind_texture(WebGlRenderingContext::TEXTURE_2D, Some(&self.word_tex));
        u1i!("uWord", 0);

        draw_quad(gl, &self.quad_buf, &self.prog);
    }

    fn destroy(&self) {
        let gl = &self.gl;
        gl.delete_program(Some(&self.prog));
        gl.delete_shader(Some(&self.vs));
        gl.delete_shader(Some(&self.fs));
        gl.delete_buffer(Some(&self.quad_buf));
        gl.delete_texture(Some(&self.word_tex));
        if let Some(ext) = &self.lose_ext {
            ext.lose_context();
        }
    }
}

// ── SoundingsThinking component ─────────────────────────────────────────

/// A 150×44 WebGL plate rendering the Soundings morphing field with a
/// pulsing emitter and "thinking…" callout.  Degrades to a quiet static pill
/// when WebGL is unavailable or the user prefers reduced motion.  GL
/// resources are freed on unmount (rAF cancelled, program / buffers /
/// textures deleted, context lost).
#[component]
pub fn SoundingsThinking() -> impl IntoView {
    let uid = UID.fetch_add(1, Ordering::Relaxed);
    let plate_id = format!("soundings-thinking-{uid}");

    let reduce = web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .map(|m| m.matches())
        .unwrap_or(false);

    // Probe WebGL availability with a temporary canvas.
    let gl_ok = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            let c: HtmlCanvasElement = d.create_element("canvas").ok()?.dyn_into().ok()?;
            c.get_context("webgl").ok()??;
            Some(true)
        })
        .unwrap_or(false);

    // ---- Animated path: WebGL available, no reduced-motion preference ----
    if !reduce && gl_ok {
        let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
        let holder: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let engine: Rc<RefCell<Option<Engine>>> = Rc::new(RefCell::new(None));
        let initialised: Rc<std::cell::Cell<bool>> =
            Rc::new(std::cell::Cell::new(false));
        let gl_failed: Rc<std::cell::Cell<bool>> = Rc::new(std::cell::Cell::new(false));

        let plate_inner = plate_id.clone();
        let engine_inner = engine.clone();
        let init_inner = initialised.clone();
        let _failed_inner = gl_failed.clone();
        let holder_inner = holder.clone();
        let raf_inner = raf_id.clone();

        let cb = Closure::wrap(Box::new(move |ts: f64| {
            if gl_failed.get() {
                return;
            }
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            let Some(_plate) = doc.get_element_by_id(&plate_inner) else {
                return;
            };

            // Lazy initialisation on first frame (DOM must be laid out so
            // clientWidth/Height are accurate for viewport sizing).
            if !init_inner.get() {
                init_inner.set(true);
                match init_engine(&doc, &plate_inner) {
                    Ok(eng) => *engine_inner.borrow_mut() = Some(eng),
                    Err(_) => {
                        gl_failed.set(true);
                        return;
                    }
                }
            }

            if gl_failed.get() {
                return;
            }

            let mut eng_ref = engine_inner.borrow_mut();
            if let Some(eng) = eng_ref.as_mut() {
                let t = ts / 1000.0;
                eng.render(t);
            }

            // Self-reschedule.
            if let Some(c) = holder_inner.borrow().as_ref() {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    *raf_inner.borrow_mut() = id;
                }
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
                *raf_id.borrow_mut() = id;
            }
        }
        *holder.borrow_mut() = Some(cb);

        let cleanup = send_wrapper::SendWrapper::new((raf_id, holder, engine));
        on_cleanup(move || {
            let (raf_id, holder, engine) = &*cleanup;
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(*raf_id.borrow());
            }
            holder.borrow_mut().take();
            if let Some(eng) = engine.borrow_mut().take() {
                eng.destroy();
            }
        });
    }

    // ---- Settled path: reduced motion — draw ONE frame with t frozen ----
    if reduce && gl_ok {
        let settled: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let engine_settle: Rc<RefCell<Option<Engine>>> = Rc::new(RefCell::new(None));
        let plate_settle = plate_id.clone();
        let engine_clone = engine_settle.clone();
        let settled_clone = settled.clone();

        // Use a one-shot rAF to draw after layout.
        let settle_cb = Closure::wrap(Box::new(move |_ts: f64| {
            if settled_clone.get() {
                return;
            }
            settled_clone.set(true);
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            if doc.get_element_by_id(&plate_settle).is_none() {
                return;
            }
            match init_engine(&doc, &plate_settle) {
                Ok(mut eng) => {
                    // Pick a visually settled t — the emitter pulse has
                    // passed and the morphing field is idling.
                    eng.render(3.5);
                    *engine_clone.borrow_mut() = Some(eng);
                }
                Err(_) => {
                    // Degrade — the div fallback text covers us.
                }
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            let _ = win.request_animation_frame(settle_cb.as_ref().unchecked_ref());
        }

        let cleanup = send_wrapper::SendWrapper::new((settle_cb, engine_settle));
        on_cleanup(move || {
            let (_cb, engine) = &*cleanup;
            if let Some(eng) = engine.borrow_mut().take() {
                eng.destroy();
            }
        });
    }

    // ---- View — always a 150×44 plate, GL canvas layered on top ----
    let radius = (44.0_f64 * 0.23).round() as u32;
    let plate_style = format!(
        "position:relative;width:150px;height:44px;border-radius:{radius}px;\
         overflow:hidden;border:1px solid rgba(255,255,255,0.10);\
         background:var(--bg-elevated);flex:none"
    );

    view! {
        <div id=plate_id class="soundings-thinking" style=plate_style>
            // Degrade pill — shown when WebGL never starts (canvas
            // absent).  When GL is alive the canvas covers it.
            <span class="soundings-thinking__degrade" style="
                position:absolute;inset:0;display:flex;align-items:center;
                justify-content:center;color:var(--fg-3);
                font-family:var(--wm-font),Poppins,system-ui;
                font-weight:600;font-size:14px;line-height:1;
            ">"thinking…"</span>
        </div>
    }
}

// ── One-shot engine builder (called from rAF callback) ─────────────────

fn init_engine(doc: &web_sys::Document, plate_id: &str) -> Result<Engine, String> {
    // Read token colours from :root computed style (zero hex in Rust).
    let indigo = read_token_vec3("--ocean-2");
    let cy = read_token_vec3("--ocean-6");
    let hot = read_token_vec3("--ocean-7");
    let foam = read_token_vec3("--badge-foam");
    let dim = read_token_vec3("--fg-3");
    let bg = read_token_vec3("--bg-elevated");

    // Create the GL canvas and append it to the plate.
    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .map_err(|_| "createElement")?
        .dyn_into()
        .map_err(|_| "HtmlCanvasElement")?;
    canvas
        .set_attribute(
            "style",
            "position:absolute;inset:0;width:100%;height:100%;display:block",
        )
        .map_err(|_| "setAttribute")?;

    let plate = doc
        .get_element_by_id(plate_id)
        .ok_or("plate not found")?;
    plate
        .append_child(&canvas)
        .map_err(|_| "appendChild")?;

    // Initialise WebGL.
    let gl = init_gl(&canvas)?;

    let vs = compile_shader(&gl, WebGlRenderingContext::VERTEX_SHADER, QUAD_VS)?;
    let fs = compile_shader(&gl, WebGlRenderingContext::FRAGMENT_SHADER, THINKING_FS)
        .map_err(|e| {
            gl.delete_shader(Some(&vs));
            e
        })?;
    let prog = link_program(&gl, &vs, &fs).map_err(|e| {
        gl.delete_shader(Some(&vs));
        gl.delete_shader(Some(&fs));
        e
    })?;
    let uniforms = build_uniform_map(&gl, &prog);
    let quad_buf = create_fullscreen_quad(&gl);
    let word = make_thinking_texture(&gl).map_err(|e| {
        gl.delete_program(Some(&prog));
        gl.delete_shader(Some(&vs));
        gl.delete_shader(Some(&fs));
        gl.delete_buffer(Some(&quad_buf));
        e
    })?;

    let lose_ext: Option<WebglLoseContext> = gl
        .get_extension("WEBGL_lose_context")
        .ok()
        .flatten()
        .map(|v| v.unchecked_into());

    Ok(Engine {
        gl,
        prog,
        vs,
        fs,
        quad_buf,
        word_tex: word.tex,
        word_aspect: word.aspect,
        uniforms,
        canvas,
        lose_ext,
        tok_indigo: indigo,
        tok_cy: cy,
        tok_hot: hot,
        tok_foam: foam,
        tok_dim: dim,
        tok_bg: bg,
        ping: 0.4,
        next_ping: 2.2,
    })
}

// ── Hero landing choreography ────────────────────────────────────────────

/// Tuning constants: TUNE.hero from soundings2.js, verbatim.
const LINE_SCALE: f32 = 3.4;
const LINE_EXP: f32 = 26.0;
const CHROMA: f32 = 1.0;
const FLASH: f32 = 0.65;
const KICK_FACTOR: f32 = 0.28;
const GAIN: f32 = 1.0;
const QUIET: f32 = 1.0;
const MAXE: usize = 10;
const INTRO: f32 = 3.0;

/// Sessions launcher button position in p-space (warp-space input).
const BTN_P: (f32, f32) = (0.0, -0.27);

/// Letter positions in p-space — 5 letters O C E A N, centred.
const L_GAP: f32 = 0.245;
const L_Y: f32 = 0.16;
const L_HALF: f32 = 0.115;

fn warp_p(x: f32, y: f32) -> (f32, f32) {
    (x * (1.0 + y * 0.45), y * 1.35)
}

fn clamp01(t: f32) -> f32 { t.clamp(0.0, 1.0) }

fn out_cubic(t: f32) -> f32 {
    let u = clamp01(t);
    1.0 - (1.0 - u).powi(3)
}

fn in_cubic(t: f32) -> f32 {
    let u = clamp01(t);
    u * u * u
}

struct Chor {
    t1: f32,
    c: f32,
    decay: f32,
    kick_decay: f32,
    idle_from: f32,
}

#[derive(Clone)]
struct Evt {
    x: f32,
    y: f32,
    t0: f32,
    amp: f32,
    ring: bool,
}

struct BeadPlan {
    x: f32,
    y_top: f32,
    y_hit: f32,
    hold_from: f32,
    fall_from: f32,
    t0: f32,
}

struct LetterState {
    etch_t: f32,
    lit: f32,
    boost: f32,
}

fn schedule() -> (Chor, Vec<Evt>, BeadPlan, f32) {
    let i = INTRO;
    let cl = |v: f32, a: f32, b: f32| v.clamp(a, b);
    let hold = cl(0.18 * i, 0.22, 0.60);
    let drop = cl(0.06 * i, 0.14, 0.24);
    let t1 = hold + drop;
    let chor = Chor {
        t1,
        c: 0.62 * cl(3.0 / i, 0.8, 1.2),
        decay: 0.9 * cl(3.0 / i, 0.85, 1.2),
        kick_decay: 3.2 * cl(3.0 / i, 1.0, 1.2),
        idle_from: i + 1.3,
    };

    // Strikes plan (unwarped p-space)
    let strike_plan: [(f32, f32, f32, f32); 3] = [
        (-0.28, -0.06, t1, 1.0),
        (0.34, 0.12, t1 + 0.16 * i, 0.75),
        (0.06, -0.30, t1 + 0.30 * i, 0.55),
    ];

    let mut events: Vec<Evt> = Vec::new();
    for &(sx, sy, st0, samp) in &strike_plan {
        let (wx, wy) = warp_p(sx, sy);
        events.push(Evt {
            x: wx,
            y: wy,
            t0: st0,
            amp: samp,
            ring: false,
        });
    }

    let bead = BeadPlan {
        x: -0.28,
        y_top: 0.42,
        y_hit: -0.06,
        hold_from: 0.05,
        fall_from: hold,
        t0: t1,
    };
    let idle_from = chor.idle_from;

    (chor, events, bead, idle_from)
}

// ── Letter texture rasterisation ─────────────────────────────────────────

struct LetterTextures {
    tex: [WebGlTexture; 5],
}

/// Rasterise each of O C E A N at 256×256 into per-letter GL textures,
/// filled with --ocean-4..8 token colours (read via getComputedStyle).
fn make_letter_textures(
    gl: &WebGlRenderingContext,
    doc: &web_sys::Document,
) -> Result<LetterTextures, String> {
    let font_family = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| {
            web_sys::window()
                .and_then(|w| w.get_computed_style(&el).ok())
                .flatten()
        })
        .and_then(|style| style.get_property_value("--wm-font").ok())
        .unwrap_or_else(|| "'Poppins', system-ui, sans-serif".into());

    let token_names: [&str; 5] = ["--ocean-4", "--ocean-5", "--ocean-6", "--ocean-7", "--ocean-8"];
    let letters: [char; 5] = ['O', 'C', 'E', 'A', 'N'];

    let mut tex_arr: [Option<WebGlTexture>; 5] = [None, None, None, None, None];

    for i in 0..5 {
        let canvas: HtmlCanvasElement = doc
            .create_element("canvas")
            .map_err(|_| "createElement")?
            .dyn_into()
            .map_err(|_| "HtmlCanvasElement cast")?;
        canvas.set_width(256);
        canvas.set_height(256);

        let ctx: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .map_err(|_| "getContext 2d")?
            .ok_or("no 2d context")?
            .dyn_into()
            .map_err(|_| "CanvasRenderingContext2d cast")?;

        let font = format!("700 172px {}", font_family);
        ctx.set_font(&font);
        ctx.set_text_align("center");
        ctx.set_text_baseline("middle");

        // Read the token fill colour as a CSS string.
        let fill = doc
            .document_element()
            .and_then(|el| {
                web_sys::window()
                    .and_then(|w| w.get_computed_style(&el).ok())
                    .flatten()
            })
            .and_then(|style| style.get_property_value(token_names[i]).ok())
            .unwrap_or_else(|| "#ffffff".into());

        ctx.set_fill_style_str(&fill);
        ctx
            .fill_text(&letters[i].to_string(), 128.0, 140.0)
            .map_err(|_| "fillText")?;

        tex_arr[i] = Some(make_texture(gl, &canvas));
    }

    // Unwrap all Options — they were all set to Some in the loop.
    let tex = [
        tex_arr[0].take().unwrap(),
        tex_arr[1].take().unwrap(),
        tex_arr[2].take().unwrap(),
        tex_arr[3].take().unwrap(),
        tex_arr[4].take().unwrap(),
    ];

    Ok(LetterTextures { tex })
}

// ── SoundingsLanding engine ──────────────────────────────────────────────

/// Complete GL state for the hero landing scene: field shader + letter
/// shader, 5 letter textures, choreography bookkeeping.
struct SoundingsLandingEngine {
    gl: WebGlRenderingContext,
    field_prog: WebGlProgram,
    field_vs: WebGlShader,
    field_fs: WebGlShader,
    letter_prog: WebGlProgram,
    letter_vs: WebGlShader,
    letter_fs: WebGlShader,
    quad_buf: WebGlBuffer,
    letter_tex: [WebGlTexture; 5],
    field_uniforms: HashMap<String, WebGlUniformLocation>,
    letter_uniforms: HashMap<String, WebGlUniformLocation>,
    canvas: HtmlCanvasElement,
    lose_ext: Option<WebglLoseContext>,
    // Token colours resolved from CSS.
    tok_indigo: [f32; 3],
    tok_cy: [f32; 3],
    tok_hot: [f32; 3],
    // Choreography.
    events: Vec<Evt>,
    chor: Chor,
    bead: BeadPlan,
    letters: [LetterState; 5],
    next_idle: f32,
    idle_count: u32,
    revealed: bool,
    glint_t: f32,
    orb_x: f32,
    orb_y: f32,
    el: web_sys::Element,
    t0: f64,
}

impl SoundingsLandingEngine {
    fn render(&mut self, now: f64) {
        // t0 latches from the first rAF timestamp (performance-origin);
        // an epoch t0 would push uT past f32 precision and freeze the field.
        if self.t0 < 0.0 {
            self.t0 = now;
        }
        let t = (now - self.t0) as f32 / 1000.0;
        let dt = 0.016; // ~60 fps fixed step
        let en = 1.0_f32;

        // ── idle pings ───────────────────────────────────────────────
        if t >= self.next_idle {
            self.idle_count += 1;
            let from_orb = self.idle_count % 3 != 0;
            let ix = if from_orb {
                self.orb_x
            } else {
                (js_sys::Math::random() as f32 * 2.0 - 1.0) * 0.5
            };
            let iy = if from_orb {
                self.orb_y - 0.02
            } else {
                (js_sys::Math::random() as f32 * 2.0 - 1.0) * 0.28
            };
            let amp = (0.20 + js_sys::Math::random() as f32 * 0.20)
                * (0.55 + 0.45 * clamp01((t - self.chor.idle_from) / 5.0));
            let (wx, wy) = warp_p(ix, iy);
            self.events.push(Evt {
                x: wx,
                y: wy,
                t0: t + 0.05,
                amp,
                ring: false,
            });
            if from_orb {
                self.glint_t = t + 0.05;
            }
            self.next_idle = t + 2.4 + js_sys::Math::random() as f32 * 2.2;
        }
        // Cap events at 9s lifetime.
        self.events.retain(|e| t - e.t0 < 9.0);

        // ── letter etches ────────────────────────────────────────────
        let letter_p: [(f32, f32); 5] = [
            (-2.0 * L_GAP, L_Y),
            (-1.0 * L_GAP, L_Y),
            (0.0, L_Y),
            (1.0 * L_GAP, L_Y),
            (2.0 * L_GAP, L_Y),
        ];

        let mut ring_backs: Vec<Evt> = Vec::new();

        for i in 0..5 {
            let (lx, ly) = warp_p(letter_p[i].0, letter_p[i].1);
            let l = &mut self.letters[i];
            let mut boost: f32 = 0.0;

            for e in &self.events {
                let te = t - e.t0;
                if te <= 0.0 {
                    continue;
                }
                let r = ((lx - e.x).powi(2) + (ly - e.y).powi(2)).sqrt();
                let front = self.chor.c * te - r;
                if !e.ring && e.amp >= 0.5 && l.etch_t < 0.0 && front > 0.0 {
                    l.etch_t = t;
                    ring_backs.push(Evt {
                        x: lx,
                        y: ly,
                        t0: t + 0.03,
                        amp: 0.15,
                        ring: true,
                    });
                }
                boost += (-front * front * 55.0).exp() * e.amp * 0.12;
            }
            if l.etch_t >= 0.0 {
                l.lit = (l.lit + dt / 0.15).min(1.0);
            }
            l.boost = boost.min(0.22);
        }
        self.events.extend(ring_backs);

        // ── launcher reveal ──────────────────────────────────────────
        if !self.revealed {
            let (bx, by) = warp_p(BTN_P.0, BTN_P.1);
            for e in &self.events {
                if e.ring || e.amp < 0.5 {
                    continue;
                }
                let te = t - e.t0;
                if te > 0.0
                    && self.chor.c * te > ((bx - e.x).powi(2) + (by - e.y).powi(2)).sqrt()
                {
                    self.revealed = true;
                    let _ = self.el.set_attribute("data-flooded", "");
                    break;
                }
            }
        }

        // ── exposure kick ────────────────────────────────────────────
        let mut kick: f32 = 0.0;
        for e in &self.events {
            if e.ring || e.amp < 0.5 {
                continue;
            }
            let te = t - e.t0;
            if te > 0.0 {
                kick += e.amp * (-te * self.chor.kick_decay).exp();
            }
        }
        kick = (kick * KICK_FACTOR).min(0.7);

        // ── bead: hold, breathe, drop ────────────────────────────────
        let mut src: [f32; 4] = [0.0, 0.0, 0.0, 0.02];
        {
            let b = &self.bead;
            if t >= b.hold_from && t < b.t0 + 0.05 {
                let fade = clamp01((t - b.hold_from) / 0.35);
                let (y, glow) = if t < b.fall_from {
                    let pre = clamp01((t - (b.fall_from - 0.12)) / 0.12);
                    let y =
                        b.y_top + 0.012 * pre * pre;
                    let glow = fade
                        * (0.45
                            + 0.22
                                * ((t - b.hold_from) * 2.0 * std::f32::consts::PI * 1.0).sin()
                            + 0.35 * pre * pre);
                    (y, glow)
                } else {
                    let f = in_cubic((t - b.fall_from) / (b.t0 - b.fall_from).max(0.05));
                    let y = (b.y_top + 0.012) + (b.y_hit - (b.y_top + 0.012)) * f;
                    (y, 0.9)
                };
                src = [b.x, y, glow, 0.013 + 0.005 * glow];
            }
            // Idle pings from strike point; source re-glows softly.
            self.orb_x = b.x;
            self.orb_y = b.y_hit;
            if self.glint_t >= 0.0 && t >= self.glint_t {
                let g = (-(t - self.glint_t) * 3.0).exp();
                if g > src[2] {
                    src = [self.orb_x, self.orb_y, 0.55 * g, 0.016];
                }
            }
        }

        // ── upload events ────────────────────────────────────────────
        let slice_len = self.events.len().min(MAXE);
        let start = self.events.len().saturating_sub(MAXE);
        let mut evt_arr = [0.0_f32; MAXE * 4];
        for i in 0..slice_len {
            let e = &self.events[start + i];
            evt_arr[i * 4] = e.x;
            evt_arr[i * 4 + 1] = e.y;
            evt_arr[i * 4 + 2] = e.t0;
            evt_arr[i * 4 + 3] = e.amp * en;
        }

        // ── wake gating ──────────────────────────────────────────────
        let wake = 0.55 + 0.45 * out_cubic(t / (self.chor.t1 + 0.5));

        // ── draw field ───────────────────────────────────────────────
        let gl = &self.gl;
        let w = self.canvas.width();
        let h = self.canvas.height();

        gl.viewport(0, 0, w as i32, h as i32);
        gl.disable(WebGlRenderingContext::BLEND);
        gl.use_program(Some(&self.field_prog));

        macro_rules! u1f {
            ($name:expr, $val:expr) => {
                if let Some(loc) = self.field_uniforms.get($name) {
                    gl.uniform1f(Some(loc), $val);
                }
            };
        }
        macro_rules! u2f {
            ($name:expr, $x:expr, $y:expr) => {
                if let Some(loc) = self.field_uniforms.get($name) {
                    gl.uniform2f(Some(loc), $x, $y);
                }
            };
        }
        macro_rules! u3fv {
            ($name:expr, $v:expr) => {
                if let Some(loc) = self.field_uniforms.get($name) {
                    gl.uniform3fv_with_f32_array(Some(loc), &$v);
                }
            };
        }
        macro_rules! u4fv {
            ($name:expr, $v:expr) => {
                if let Some(loc) = self.field_uniforms.get($name) {
                    gl.uniform4fv_with_f32_array(Some(loc), &$v);
                }
            };
        }

        u2f!("uRes", w as f32, h as f32);
        u1f!("uT", t);
        u1f!("uEn", en);
        u4fv!("uEvt", evt_arr);
        u2f!("uSrc", src[0], src[1]);
        u1f!("uKick", kick);
        u1f!("uLineScale", LINE_SCALE);
        u1f!("uLineExp", LINE_EXP);
        u1f!("uChroma", CHROMA);
        u1f!("uGain", GAIN);
        u1f!("uQuiet", QUIET * wake);
        u1f!("uC", self.chor.c);
        u1f!("uDecay", self.chor.decay);
        u3fv!("uIndigo", self.tok_indigo);
        u3fv!("uCy", self.tok_cy);
        u3fv!("uHot", self.tok_hot);
        // uSrc.z (glow) and uSrc.w (radius) set via uniform4f
        if let Some(loc) = self.field_uniforms.get("uSrc") {
            gl.uniform4f(Some(loc), src[0], src[1], src[2], src[3]);
        }

        draw_quad(gl, &self.quad_buf, &self.field_prog);

        // ── draw letters ─────────────────────────────────────────────
        gl.enable(WebGlRenderingContext::BLEND);
        gl.blend_func(
            WebGlRenderingContext::SRC_ALPHA,
            WebGlRenderingContext::ONE_MINUS_SRC_ALPHA,
        );
        gl.use_program(Some(&self.letter_prog));

        let aspect = w as f32 / h as f32;
        for i in 0..5 {
            let l = &self.letters[i];
            let d = if l.etch_t >= 0.0 {
                t - l.etch_t
            } else {
                -1.0
            };
            let flash = if d >= 0.0 {
                (-d * 4.0).exp() * FLASH
            } else {
                0.0
            };
            let split = if d >= 0.0 {
                0.010 * (-d * 5.0).exp() * FLASH
            } else {
                0.0
            };
            let pop = if d >= 0.0 {
                1.0 + 0.05 * (-d * 6.0).exp() * (2.0 * std::f32::consts::PI * 2.2 * d).cos()
            } else {
                1.0
            };
            // Brand shimmer once settled.
            let ph = ((t + i as f32 * 0.55) % 5.5) / 5.5;
            let shim = if l.lit > 0.98 {
                ((ph * std::f32::consts::PI).sin()).max(0.0) * 0.14
            } else {
                0.0
            };
            let half = L_HALF * pop;

            macro_rules! letter_u1f {
                ($name:expr, $val:expr) => {
                    if let Some(loc) = self.letter_uniforms.get($name) {
                        gl.uniform1f(Some(loc), $val);
                    }
                };
            }
            macro_rules! letter_u2f {
                ($name:expr, $x:expr, $y:expr) => {
                    if let Some(loc) = self.letter_uniforms.get($name) {
                        gl.uniform2f(Some(loc), $x, $y);
                    }
                };
            }
            macro_rules! letter_u1i {
                ($name:expr, $val:expr) => {
                    if let Some(loc) = self.letter_uniforms.get($name) {
                        gl.uniform1i(Some(loc), $val);
                    }
                };
            }

            letter_u2f!("uPos", 2.0 * letter_p[i].0 / aspect, 2.0 * letter_p[i].1);
            letter_u2f!("uScl", 2.0 * half / aspect, 2.0 * half);
            letter_u1f!("uLit", (l.lit + l.boost + shim).min(1.0));
            letter_u1f!("uFlash", flash.min(1.0));
            letter_u1f!("uSplit", split);

            gl.active_texture(WebGlRenderingContext::TEXTURE0);
            gl.bind_texture(WebGlRenderingContext::TEXTURE_2D, Some(&self.letter_tex[i]));
            letter_u1i!("uTex", 0);

            draw_quad(gl, &self.quad_buf, &self.letter_prog);
        }
    }

    fn destroy(&self) {
        let gl = &self.gl;
        gl.delete_program(Some(&self.field_prog));
        gl.delete_program(Some(&self.letter_prog));
        gl.delete_shader(Some(&self.field_vs));
        gl.delete_shader(Some(&self.field_fs));
        gl.delete_shader(Some(&self.letter_vs));
        gl.delete_shader(Some(&self.letter_fs));
        gl.delete_buffer(Some(&self.quad_buf));
        for t in &self.letter_tex {
            gl.delete_texture(Some(t));
        }
        if let Some(ext) = &self.lose_ext {
            ext.lose_context();
        }
    }
}

// ── SoundingsLanding component ───────────────────────────────────────────

/// Full-viewport WebGL hero landing scene — the Soundings morphing field
/// with dispersive wave physics, letter etches, and launcher reveal.
/// Degrades to a static settled frame when the user prefers reduced motion
/// or WebGL is unavailable.
#[component]
pub fn SoundingsLanding() -> impl IntoView {
    let uid = UID.fetch_add(1, Ordering::Relaxed);
    let host_id = format!("soundings-landing-{uid}");

    let reduce = web_sys::window()
        .and_then(|w| {
            w.match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
        })
        .map(|m| m.matches())
        .unwrap_or(false);

    let gl_ok = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            let c: HtmlCanvasElement = d.create_element("canvas").ok()?.dyn_into().ok()?;
            c.get_context("webgl").ok()??;
            Some(true)
        })
        .unwrap_or(false);

    // Pre-compute data-flooded state: reduced-motion/no-GL → revealed immediately.
    let flood_now = reduce || !gl_ok;

    // ── Animated path ────────────────────────────────────────────────
    if !reduce && gl_ok {
        let raf_id: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
        let holder: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
        let engine: Rc<RefCell<Option<SoundingsLandingEngine>>> = Rc::new(RefCell::new(None));
        let initialised: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let gl_failed: Rc<Cell<bool>> = Rc::new(Cell::new(false));

        let host_inner = host_id.clone();
        let engine_inner = engine.clone();
        let init_inner = initialised.clone();
        let _failed_inner = gl_failed.clone();
        let holder_inner = holder.clone();
        let raf_inner = raf_id.clone();

        let cb = Closure::wrap(Box::new(move |ts: f64| {
            if gl_failed.get() {
                return;
            }
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            let Some(host_el) = doc.get_element_by_id(&host_inner) else {
                return;
            };

            if !init_inner.get() {
                init_inner.set(true);
                match init_landing_engine(&doc, &host_el) {
                    Ok(eng) => *engine_inner.borrow_mut() = Some(eng),
                    Err(_) => {
                        gl_failed.set(true);
                        return;
                    }
                }
            }

            if gl_failed.get() {
                return;
            }

            let mut eng_ref = engine_inner.borrow_mut();
            if let Some(eng) = eng_ref.as_mut() {
                eng.render(ts);
            }

            if let Some(c) = holder_inner.borrow().as_ref() {
                if let Ok(id) = win.request_animation_frame(c.as_ref().unchecked_ref()) {
                    *raf_inner.borrow_mut() = id;
                }
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            if let Ok(id) = win.request_animation_frame(cb.as_ref().unchecked_ref()) {
                *raf_id.borrow_mut() = id;
            }
        }
        *holder.borrow_mut() = Some(cb);

        let cleanup = send_wrapper::SendWrapper::new((raf_id, holder, engine));
        on_cleanup(move || {
            let (raf_id, holder, engine) = &*cleanup;
            if let Some(w) = web_sys::window() {
                let _ = w.cancel_animation_frame(*raf_id.borrow());
            }
            holder.borrow_mut().take();
            if let Some(eng) = engine.borrow_mut().take() {
                eng.destroy();
            }
        });
    }

    // ── Settled path: reduced-motion, draw one frame ─────────────────
    if reduce && gl_ok {
        let settled: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let engine_settle: Rc<RefCell<Option<SoundingsLandingEngine>>> =
            Rc::new(RefCell::new(None));
        let host_settle = host_id.clone();
        let engine_clone = engine_settle.clone();
        let settled_clone = settled.clone();

        let settle_cb = Closure::wrap(Box::new(move |_ts: f64| {
            if settled_clone.get() {
                return;
            }
            settled_clone.set(true);
            let Some(win) = web_sys::window() else { return };
            let Some(doc) = win.document() else { return };
            let Some(host_el) = doc.get_element_by_id(&host_settle) else {
                return;
            };
            match init_landing_engine(&doc, &host_el) {
                Ok(mut eng) => {
                    // Frozen mid-scene: letters lit, field idle. Step a few
                    // fixed frames so per-frame accumulators (letter lit,
                    // launcher reveal) converge before freezing.
                    eng.t0 = 0.0;
                    for i in 0..40 {
                        eng.render(4500.0 + f64::from(i) * 16.0);
                    }
                    *engine_clone.borrow_mut() = Some(eng);
                }
                Err(_) => {}
            }
        }) as Box<dyn FnMut(f64)>);

        if let Some(win) = web_sys::window() {
            let _ = win.request_animation_frame(settle_cb.as_ref().unchecked_ref());
        }

        let cleanup = send_wrapper::SendWrapper::new((settle_cb, engine_settle));
        on_cleanup(move || {
            let (_cb, engine) = &*cleanup;
            if let Some(eng) = engine.borrow_mut().take() {
                eng.destroy();
            }
        });
    }

    view! {
        <div
            id=host_id
            class="soundings-landing"
            style="position:absolute;inset:0;overflow:hidden"
            data-flooded=if flood_now { Some("") } else { None }
        >
            // Canvas injected at rAF init; empty div covers
            // the no-GL fallback (no visual content needed — the h1
            // and launcher carry the brand).
        </div>
    }
}

/// One-shot landing engine builder (called from rAF callback).
fn init_landing_engine(
    doc: &web_sys::Document,
    host: &web_sys::Element,
) -> Result<SoundingsLandingEngine, String> {
    let indigo = read_token_vec3("--ocean-2");
    let cy = read_token_vec3("--ocean-6");
    let hot = read_token_vec3("--ocean-7");

    let canvas: HtmlCanvasElement = doc
        .create_element("canvas")
        .map_err(|_| "createElement")?
        .dyn_into()
        .map_err(|_| "HtmlCanvasElement")?;
    canvas
        .set_attribute(
            "style",
            "position:absolute;inset:0;width:100%;height:100%;display:block",
        )
        .map_err(|_| "setAttribute")?;
    host
        .append_child(&canvas)
        .map_err(|_| "appendChild")?;

    let gl = init_gl(&canvas)?;

    // Compile field program.
    let field_vs =
        compile_shader(&gl, WebGlRenderingContext::VERTEX_SHADER, QUAD_VS)?;
    let field_fs =
        compile_shader(&gl, WebGlRenderingContext::FRAGMENT_SHADER, HERO_FS)
            .map_err(|e| {
                gl.delete_shader(Some(&field_vs));
                e
            })?;
    let field_prog = link_program(&gl, &field_vs, &field_fs).map_err(|e| {
        gl.delete_shader(Some(&field_vs));
        gl.delete_shader(Some(&field_fs));
        e
    })?;
    let field_uniforms = build_uniform_map(&gl, &field_prog);

    // Compile letter program.
    let letter_vs =
        compile_shader(&gl, WebGlRenderingContext::VERTEX_SHADER, LETTER_VS)?;
    let letter_fs =
        compile_shader(&gl, WebGlRenderingContext::FRAGMENT_SHADER, LETTER_FS)
            .map_err(|e| {
                gl.delete_shader(Some(&letter_vs));
                e
            })?;
    let letter_prog = link_program(&gl, &letter_vs, &letter_fs).map_err(|e| {
        gl.delete_shader(Some(&letter_vs));
        gl.delete_shader(Some(&letter_fs));
        e
    })?;
    let letter_uniforms = build_uniform_map(&gl, &letter_prog);

    let quad_buf = create_fullscreen_quad(&gl);
    let ltx = make_letter_textures(&gl, doc).map_err(|e| {
        gl.delete_program(Some(&field_prog));
        gl.delete_program(Some(&letter_prog));
        gl.delete_shader(Some(&field_vs));
        gl.delete_shader(Some(&field_fs));
        gl.delete_shader(Some(&letter_vs));
        gl.delete_shader(Some(&letter_fs));
        gl.delete_buffer(Some(&quad_buf));
        e
    })?;

    let (chor, events, bead, next_idle) = schedule();
    let (bead_orb_x, bead_orb_y) = (bead.x, bead.y_hit);

    let lose_ext: Option<WebglLoseContext> = gl
        .get_extension("WEBGL_lose_context")
        .ok()
        .flatten()
        .map(|v| v.unchecked_into());

    Ok(SoundingsLandingEngine {
        gl,
        field_prog,
        field_vs,
        field_fs,
        letter_prog,
        letter_vs,
        letter_fs,
        quad_buf,
        letter_tex: ltx.tex,
        field_uniforms,
        letter_uniforms,
        canvas,
        lose_ext,
        tok_indigo: indigo,
        tok_cy: cy,
        tok_hot: hot,
        events,
        chor,
        bead,
        letters: [
            LetterState {
                etch_t: -1.0,
                lit: 0.0,
                boost: 0.0,
            },
            LetterState {
                etch_t: -1.0,
                lit: 0.0,
                boost: 0.0,
            },
            LetterState {
                etch_t: -1.0,
                lit: 0.0,
                boost: 0.0,
            },
            LetterState {
                etch_t: -1.0,
                lit: 0.0,
                boost: 0.0,
            },
            LetterState {
                etch_t: -1.0,
                lit: 0.0,
                boost: 0.0,
            },
        ],
        next_idle,
        idle_count: 0,
        revealed: false,
        glint_t: -1.0,
        orb_x: bead_orb_x,
        orb_y: bead_orb_y,
        el: host.clone(),
        t0: -1.0,
    })
}

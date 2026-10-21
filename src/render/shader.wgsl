struct Scene {
    out_size: vec2f,
    aspect: f32,
    radius: f32,
    content: vec4f,      // x, y, w, h in canvas units (canvas is aspect x 1)
    cam: vec4f,          // cx, cy, scale, shadow strength
    prev_cam: vec4f,     // cx, cy, scale, blur sample count
    bg0: vec4f,
    bg1: vec4f,
    bg2: vec4f,
    bg3: vec4f,
    params: vec4f,       // video lod, bg kind (0 colour, 1 gradient, 2 image), bicubic flag, bg lod
};

@group(0) @binding(0) var<uniform> u: Scene;
@group(0) @binding(1) var video: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var bg_image: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.pos = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2f(p.x, 1.0 - p.y);
    return o;
}

fn sd_round_rect(p: vec2f, half: vec2f, r: f32) -> f32 {
    let q = abs(p) - half + vec2f(r);
    return length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn hash(p: vec2f) -> f32 {
    return fract(sin(dot(p, vec2f(12.9898, 78.233))) * 43758.5453);
}

fn background(q: vec2f) -> vec3f {
    let kind = u.params.y;
    if (kind < 0.5) {
        return u.bg0.rgb;
    }
    if (kind > 1.5) {
        // Image, scaled to cover the canvas.
        var t = vec2f(q.x / u.aspect, q.y);
        let ia = u.bg0.x;
        if (ia > u.aspect) {
            t.x = 0.5 + (t.x - 0.5) * u.aspect / ia;
        } else {
            t.y = 0.5 + (t.y - 0.5) * ia / u.aspect;
        }
        return textureSampleLevel(bg_image, samp, t, u.params.w).rgb;
    }
    // Soft four-corner mesh gradient with a gentle warp, like a macOS wallpaper.
    var t = vec2f(q.x / u.aspect, q.y);
    t += 0.08 * vec2f(sin(t.y * 3.1 + 1.3), cos(t.x * 2.7 + 0.4));
    t = clamp(t, vec2f(0.0), vec2f(1.0));
    let s = t * t * (3.0 - 2.0 * t);
    let top = mix(u.bg0.rgb, u.bg1.rgb, s.x);
    let bottom = mix(u.bg2.rgb, u.bg3.rgb, s.x);
    return mix(top, bottom, s.y);
}

// Catmull-Rom bicubic filter in 9 bilinear taps. Used when the video is
// magnified (zoomed in), where plain bilinear looks soft and blocky.
fn sample_bicubic(uv: vec2f) -> vec3f {
    let size = vec2f(textureDimensions(video));
    let pos = uv * size;
    let t1 = floor(pos - 0.5) + 0.5;
    let f = pos - t1;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let p0 = (t1 - 1.0) / size;
    let p3 = (t1 + 2.0) / size;
    let p12 = (t1 + w2 / w12) / size;
    var c = vec3f(0.0);
    c += textureSampleLevel(video, samp, vec2f(p0.x, p0.y), 0.0).rgb * w0.x * w0.y;
    c += textureSampleLevel(video, samp, vec2f(p12.x, p0.y), 0.0).rgb * w12.x * w0.y;
    c += textureSampleLevel(video, samp, vec2f(p3.x, p0.y), 0.0).rgb * w3.x * w0.y;
    c += textureSampleLevel(video, samp, vec2f(p0.x, p12.y), 0.0).rgb * w0.x * w12.y;
    c += textureSampleLevel(video, samp, vec2f(p12.x, p12.y), 0.0).rgb * w12.x * w12.y;
    c += textureSampleLevel(video, samp, vec2f(p3.x, p12.y), 0.0).rgb * w3.x * w12.y;
    c += textureSampleLevel(video, samp, vec2f(p0.x, p3.y), 0.0).rgb * w0.x * w3.y;
    c += textureSampleLevel(video, samp, vec2f(p12.x, p3.y), 0.0).rgb * w12.x * w3.y;
    c += textureSampleLevel(video, samp, vec2f(p3.x, p3.y), 0.0).rgb * w3.x * w3.y;
    return max(c, vec3f(0.0));
}

// Scene colour at canvas point q; `px` is the size of one output pixel in canvas units.
fn scene(q: vec2f, px: f32) -> vec3f {
    var col = background(q);
    let half = u.content.zw * 0.5;
    let center = u.content.xy + half;
    let r = min(u.radius, min(half.x, half.y));

    let shadow = u.cam.w;
    if (shadow > 0.0) {
        let blur = 0.035;
        let ds = sd_round_rect(q - center - vec2f(0.0, 0.012), half, r);
        let a = shadow * 0.7 * (1.0 - smoothstep(-blur, blur * 1.6, ds));
        col = mix(col, vec3f(0.0), a);
    }

    let d = sd_round_rect(q - center, half, r);
    let cover = clamp(0.5 - d / px, 0.0, 1.0);
    if (cover > 0.0) {
        let uv = (q - u.content.xy) / u.content.zw;
        var v: vec3f;
        if (u.params.z > 0.5) {
            v = sample_bicubic(uv);
        } else {
            v = textureSampleLevel(video, samp, uv, u.params.x).rgb;
        }
        col = mix(col, v, cover);
    }
    return col;
}

fn to_canvas(p: vec2f, cam: vec3f) -> vec2f {
    let unit = p / u.out_size.y; // output pixels -> canvas-height units
    return cam.xy + (unit - vec2f(u.aspect * 0.5, 0.5)) / cam.z;
}

@fragment
fn fs_scene(in: VsOut) -> @location(0) vec4f {
    let p = in.pos.xy;
    let n = i32(u.prev_cam.w);
    let cur = u.cam.xyz;
    if (n <= 1) {
        let px = 1.0 / (u.out_size.y * cur.z);
        return vec4f(scene(to_canvas(p, cur), px), 1.0);
    }
    // Motion blur: average the scene over the camera path within the shutter.
    // Dithered start offset hides banding.
    let prev = u.prev_cam.xyz;
    let jitter = hash(p) / f32(n);
    var acc = vec3f(0.0);
    for (var i = 0; i < n; i++) {
        let f = clamp(f32(i) / f32(n) + jitter, 0.0, 1.0);
        let cam = vec3f(mix(prev.xy, cur.xy, f), exp(mix(log(prev.z), log(cur.z), f)));
        let px = 1.0 / (u.out_size.y * cam.z);
        acc += scene(to_canvas(p, cam), px);
    }
    return vec4f(acc / f32(n), 1.0);
}

// ---- Cursor --------------------------------------------------------------

struct Sprite {
    rect: vec4f,   // x0, y0, x1, y1 in NDC
    tint: vec4f,   // opacity in w
};

@group(0) @binding(0) var<uniform> sprite: Sprite;
@group(0) @binding(1) var sprite_tex: texture_2d<f32>;
@group(0) @binding(2) var sprite_samp: sampler;

@vertex
fn vs_sprite(@builtin(vertex_index) i: u32) -> VsOut {
    let c = vec2f(f32(i & 1u), f32((i >> 1u) & 1u));
    var o: VsOut;
    o.pos = vec4f(mix(sprite.rect.xy, sprite.rect.zw, c), 0.0, 1.0);
    o.uv = c;
    return o;
}

@fragment
fn fs_sprite(in: VsOut) -> @location(0) vec4f {
    let c = textureSample(sprite_tex, sprite_samp, in.uv);
    return c * sprite.tint.w; // premultiplied
}

// ---- Mipmap generation ---------------------------------------------------

@group(0) @binding(0) var mip_src: texture_2d<f32>;
@group(0) @binding(1) var mip_samp: sampler;

@fragment
fn fs_mip(in: VsOut) -> @location(0) vec4f {
    return textureSample(mip_src, mip_samp, in.uv);
}

struct Scene {
    out_size: vec2f,
    aspect: f32,
    radius: f32,
    content: vec4f,      // x, y, w, h in canvas units (canvas is aspect x 1)
    cam: vec4f,          // cx, cy, scale, shadow strength
    prev_cam: vec4f,     // shutter start camera: cx, cy, scale, blur sample count
    next_cam: vec4f,     // shutter end camera: cx, cy, scale, unused
    blur: vec4f,         // zoom blur strength, pan blur strength, unused, unused
    mask_info: vec4f,    // mask count
    mask_rects: array<vec4f, 8>,   // video uv x0, y0, x1, y1
    mask_params: array<vec4f, 8>,  // kind (0 blur, 1 highlight), amount
    bg0: vec4f,
    bg1: vec4f,
    bg2: vec4f,
    bg3: vec4f,
    params: vec4f,       // video lod, bg kind (0 colour, 1 gradient, 2 image), bicubic flag, bg lod
    card: vec4f,         // card rect (screen plus inset frame)
    inset: vec4f,        // inset colour, opacity (0 = no inset)
    shadow: vec4f,       // offset x, y, blur radius, inner screen radius
    click: vec4f,        // click effect x, y, progress, kind (0 none, 1 circle, 2 ripple)
    click2: vec4f,       // base radius
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

// Heavy blur for "sensitive data" masks: a ring of taps on a coarse mip.
fn sample_frosted(uv: vec2f) -> vec3f {
    let size = vec2f(textureDimensions(video));
    let step = 28.0 / size;
    var c = textureSampleLevel(video, samp, uv, 5.0).rgb * 2.0;
    var w = 2.0;
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * 0.7853982;
        c += textureSampleLevel(video, samp, uv + vec2f(cos(a), sin(a)) * step, 4.5).rgb;
        w += 1.0;
    }
    return c / w;
}

// 0 outside .. 1 inside a mask rect (in video uv), with a soft rounded edge.
fn mask_cover(uv: vec2f, r: vec4f) -> f32 {
    let size = vec2f(textureDimensions(video));
    let p = uv * size;
    let lo = r.xy * size;
    let hi = r.zw * size;
    let radius = 0.012 * size.y;
    let d = sd_round_rect(p - (lo + hi) * 0.5, (hi - lo) * 0.5, radius);
    return clamp(0.5 - d / 1.5, 0.0, 1.0);
}

fn apply_masks(v_in: vec3f, uv: vec2f) -> vec3f {
    var v = v_in;
    let n = i32(u.mask_info.x);
    var dim = 0.0;
    var outside_all = 1.0;
    var any_highlight = false;
    for (var i = 0; i < n; i++) {
        let r = u.mask_rects[i];
        let prm = u.mask_params[i];
        let cover = mask_cover(uv, r);
        if (prm.x < 0.5) {
            if (cover > 0.0) {
                v = mix(v, sample_frosted(uv), cover * prm.y);
            }
        } else {
            // Several highlights: dim what lies outside all of them.
            any_highlight = true;
            outside_all = outside_all * (1.0 - cover);
            dim = max(dim, prm.y);
        }
    }
    if (any_highlight) {
        v = v * (1.0 - dim * outside_all);
    }
    return v;
}

// Scene colour at canvas point q; `px` is the size of one output pixel in canvas units.
fn scene(q: vec2f, px: f32) -> vec3f {
    var col = background(q);
    let half = u.card.zw * 0.5;
    let center = u.card.xy + half;
    let r = min(u.radius, min(half.x, half.y));

    let shadow = u.cam.w;
    if (shadow > 0.0) {
        let blur = u.shadow.z;
        let ds = sd_round_rect(q - center - u.shadow.xy, half, r);
        let a = shadow * 0.7 * (1.0 - smoothstep(-blur, blur * 1.6, ds));
        col = mix(col, vec3f(0.0), a);
    }

    var cover: f32;
    if (u.inset.w > 0.0) {
        // Inset frame: the card in the inset colour, the screen inside it.
        let dc = sd_round_rect(q - center, half, r);
        let card_cover = clamp(0.5 - dc / px, 0.0, 1.0);
        col = mix(col, u.inset.rgb, card_cover * u.inset.w);
        let ih = u.content.zw * 0.5;
        let di = sd_round_rect(q - (u.content.xy + ih), ih, min(u.shadow.w, min(ih.x, ih.y)));
        cover = clamp(0.5 - di / px, 0.0, 1.0);
    } else {
        let d = sd_round_rect(q - center, half, r);
        cover = clamp(0.5 - d / px, 0.0, 1.0);
    }
    if (cover > 0.0) {
        let uv = (q - u.content.xy) / u.content.zw;
        var v: vec3f;
        if (u.params.z > 0.5) {
            v = sample_bicubic(uv);
        } else {
            v = textureSampleLevel(video, samp, uv, u.params.x).rgb;
        }
        if (u.mask_info.x > 0.5) {
            v = apply_masks(v, uv);
        }
        col = mix(col, v, cover);
    }
    return col;
}

// Click highlight: a soft filled circle or an expanding ring.
fn click_fx(col: vec3f, q: vec2f, px: f32) -> vec3f {
    let kind = u.click.w;
    if (kind < 0.5) {
        return col;
    }
    let p = u.click.z;
    let base = u.click2.x;
    let d = length(q - u.click.xy);
    let e = 1.0 - pow(1.0 - p, 3.0);
    if (kind < 1.5) {
        let r = base * (0.45 + 0.35 * e);
        let a = clamp(0.5 - (d - r) / px, 0.0, 1.0) * 0.32 * (1.0 - p);
        return mix(col, vec3f(1.0), a);
    }
    let r = base * (0.25 + 1.05 * e);
    let w = base * 0.07 * (1.0 - 0.5 * p) + px * 0.5;
    let a = clamp(0.5 - (abs(d - r) - w) / px, 0.0, 1.0) * 0.75 * (1.0 - p);
    return mix(col, vec3f(1.0), a);
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
        let q = to_canvas(p, cur);
        return vec4f(click_fx(scene(q, px), q, px), 1.0);
    }
    // Motion blur. Between the shutter-start and shutter-end cameras each
    // pixel moves by a radial part (scale change, about the frame centre) plus
    // a translation (centre change); the two are weighted separately and the
    // scene is averaged along the result, centred on the current camera.
    // A per-pixel dithered offset hides banding.
    let a = u.prev_cam.xyz;
    let b = u.next_cam.xyz;
    let off = p / u.out_size.y - vec2f(u.aspect * 0.5, 0.5);
    let d = off * (1.0 / b.z - 1.0 / a.z) * u.blur.x + (b.xy - a.xy) * u.blur.y;
    let q0 = to_canvas(p, cur);
    let px = 1.0 / (u.out_size.y * cur.z);
    let jitter = hash(p) / f32(n);
    var acc = vec3f(0.0);
    for (var i = 0; i < n; i++) {
        let f = f32(i) / f32(n) + jitter - 0.5;
        acc += scene(q0 + d * f, px);
    }
    return vec4f(click_fx(acc / f32(n), q0, px), 1.0);
}

// ---- Cursor --------------------------------------------------------------

struct Sprite {
    a: vec4f,      // NDC corners: top-left xy, top-right zw
    b: vec4f,      // bottom-left xy, bottom-right zw
    tint: vec4f,   // opacity in w
};

@group(0) @binding(0) var<uniform> sprite: Sprite;
@group(0) @binding(1) var sprite_tex: texture_2d<f32>;
@group(0) @binding(2) var sprite_samp: sampler;

@vertex
fn vs_sprite(@builtin(vertex_index) i: u32) -> VsOut {
    let c = vec2f(f32(i & 1u), f32((i >> 1u) & 1u));
    var o: VsOut;
    var corner: vec2f;
    switch i {
        case 0u: { corner = sprite.a.xy; }
        case 1u: { corner = sprite.a.zw; }
        case 2u: { corner = sprite.b.xy; }
        default: { corner = sprite.b.zw; }
    }
    o.pos = vec4f(corner, 0.0, 1.0);
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

// An old screen (docs/design/rendering.md, Post effects): the picture bowed a little toward its
// corners, dark lines between the rows of pixels, and a darker rim. param(0) is how dark the lines
// are (0 none, 1 black), param(1) how much the picture bows.
fn effect(uv: vec2f) -> vec4f {
    let centred = uv - 0.5;
    let bow = param(1) * dot(centred, centred);
    let p = 0.5 + centred * (1.0 + bow);
    if (p.x < 0.0 || p.x > 1.0 || p.y < 0.0 || p.y > 1.0) {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
    var c = sample_frame(p).rgb;
    let row = fract(p.y * resolution().y * 0.5);
    c = c * (1.0 - param(0) * smoothstep(0.3, 0.7, abs(row - 0.5) * 2.0));
    let rim = smoothstep(0.75, 0.45, length(centred) * 1.2);
    return vec4f(c * mix(0.6, 1.0, rim), 1.0);
}

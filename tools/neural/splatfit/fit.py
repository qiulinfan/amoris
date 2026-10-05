"""Fit 3D Gaussian splats to multi-view images rendered by Amoris's own renderer.

The closed loop of docs/spec/splats.md 7.1, with no downloaded data:

    cargo run --release -p pocket-render --example splats -- --dataset DIR --views 48
    uv run fit.py DIR --out DIR/fitted.ply
    cargo run --release -p pocket-render --example splats -- --replay DIR --ply DIR/fitted.ply
    uv run fit.py DIR --score        # PSNR of the engine's renders against the references

The differentiable rasterizer is plain PyTorch (runs on Apple MPS, CUDA or the CPU): the same
projection as splat_preprocess.wgsl (EWA, the 0.3 px^2 low-pass, 3-sigma extent), Gaussians binned
into 16x16 tiles, each tile composited front to back over its K nearest Gaussians, the renderer's
background (the flat sky) behind, then the renderer's display transform (exposure and AgX, post.wgsl)
so the loss compares what the engine would show. Colors are fitted as linear radiance and written as
display-encoded SH band 0, which is what the engine's preprocess decodes (with `radiance` 1).

Densification is a simple relocation: every few hundred steps, nearly transparent Gaussians move
next to opaque ones (sampled by opacity), with their optimizer state reset. Optional L1 penalties
on opacity and scale (`--reg`, as in 3DGS-MCMC; off by default: at 0.01 they lowered the score).

Each tile composites at most K Gaussians (`--k`); the engine draws all of them, so K must cover the
busiest tile or the engine shows Gaussians the fit never saw. The log reports the share dropped.
"""

import argparse
import math
import pathlib
import struct
import time

import numpy as np
import torch
from PIL import Image

C0 = 0.28209479177387814
TILE = 16


def load_dataset(root: pathlib.Path):
    lines = [l for l in (root / "cameras.txt").read_text().splitlines() if l and not l.startswith("#")]
    head = [float(v) for v in lines[0].split()]
    w, h, fov, exposure = int(head[0]), int(head[1]), head[2], head[3]
    sky = head[4:7]
    views = []
    for l in lines[1:]:
        p = l.split()
        eye = [float(v) for v in p[1:4]]
        target = [float(v) for v in p[4:7]]
        views.append((p[0], eye, target))
    return w, h, fov, exposure, sky, views


def load_image(path: pathlib.Path) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0


def look_at(eye, target):
    """The view matrix (right-handed, -z forward, +y up), as glam's look_at_rh."""
    eye = np.asarray(eye, np.float64)
    f = np.asarray(target, np.float64) - eye
    f /= np.linalg.norm(f)
    s = np.cross(f, [0.0, 1.0, 0.0])
    s /= np.linalg.norm(s)
    u = np.cross(s, f)
    rot = np.stack([s, u, -f])
    return rot.astype(np.float32), (-rot @ eye).astype(np.float32)


def luminance(c):
    return c[..., 0] * 0.2126 + c[..., 1] * 0.7152 + c[..., 2] * 0.0722


def agx(c):
    """post.wgsl's AgX with its punchy look; c is linear HDR (..., 3)."""
    inset = torch.tensor(
        [[0.842479062253094, 0.0423282422610123, 0.0423756549057051],
         [0.0784335999999992, 0.878468636469772, 0.0784336],
         [0.0792237451477643, 0.0791661274605434, 0.879142973793104]], device=c.device)
    outset = torch.tensor(
        [[1.19687900512017, -0.0528968517574562, -0.0529716355144438],
         [-0.0980208811401368, 1.15190312990417, -0.0980434501171241],
         [-0.0990297440797205, -0.0989611768448433, 1.15107367264116]], device=c.device)
    min_ev, max_ev = -12.47393, 4.026069
    # WGSL's mat3x3f(a, b, c) takes columns: v = inset * c is c @ rows-as-given.
    v = c @ inset
    v = torch.log2(v.clamp(min=1e-10)).clamp(min_ev, max_ev)
    v = (v - min_ev) / (max_ev - min_ev)
    x2 = v * v
    x4 = x2 * x2
    v = 15.5 * x4 * x2 - 40.14 * x4 * v + 31.96 * x4 - 6.868 * x2 * v + 0.4298 * x2 + 0.1191 * v - 0.00232
    luma = luminance(v)[..., None]
    v = v.clamp(min=0.0).pow(1.35)
    v = luma + 1.4 * (v - luma)
    v = v @ outset
    return v.clamp(0.0, 1.0)


def quat_mat(q):
    q = q / q.norm(dim=-1, keepdim=True).clamp(min=1e-12)
    x, y, z, w = q.unbind(-1)
    return torch.stack([
        torch.stack([1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)], -1),
        torch.stack([2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)], -1),
        torch.stack([2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)], -1),
    ], -2)


class Gaussians(torch.nn.Module):
    def __init__(self, n: int, device, seed=1):
        super().__init__()
        g = torch.Generator().manual_seed(seed)
        half = n // 2
        # Half on the ground (y ~ 0), half in the objects' volume: the stand-in for SfM points.
        ground = torch.rand(half, 3, generator=g) * torch.tensor([5.0, 0.02, 5.0]) - torch.tensor([2.5, 0.0, 2.5])
        volume = torch.rand(n - half, 3, generator=g) * torch.tensor([3.6, 1.7, 3.4]) - torch.tensor([1.7, 0.0, 1.7])
        self.means = torch.nn.Parameter(torch.cat([ground, volume]).to(device))
        self.log_scales = torch.nn.Parameter(torch.full((n, 3), math.log(0.04)).to(device))
        q = torch.randn(n, 4, generator=g)
        self.quats = torch.nn.Parameter((q / q.norm(dim=-1, keepdim=True)).to(device))
        self.logit_opacity = torch.nn.Parameter(torch.full((n,), math.log(0.1 / 0.9)).to(device))
        self.color_raw = torch.nn.Parameter(torch.full((n, 3), math.log(math.expm1(0.4))).to(device))

    def radiance(self):
        return torch.nn.functional.softplus(self.color_raw)


STATS = {"pairs": 0, "dropped": 0}


def render(gs: Gaussians, rot, trans, w, h, fov_deg, bg_lin, exposure, k_max=1024):
    """One view, display-encoded (h, w, 3)."""
    dev = gs.means.device
    t = gs.means @ rot.T + trans
    tz = -t[:, 2]
    tan_y = math.tan(math.radians(fov_deg) / 2)
    tan_x = tan_y * w / h
    fy = h / (2 * tan_y)
    fx = fy
    zc = tz.clamp(min=1e-3)
    u = w / 2 + fx * t[:, 0] / zc
    v = h / 2 - fy * t[:, 1] / zc
    s = gs.log_scales.exp()
    m = rot @ (quat_mat(gs.quats) * s[:, None, :])
    tx = (t[:, 0] / zc).clamp(-1.3 * tan_x, 1.3 * tan_x)
    ty = (t[:, 1] / zc).clamp(-1.3 * tan_y, 1.3 * tan_y)
    zero = torch.zeros_like(zc)
    j0 = torch.stack([fx / zc, zero, fx * tx / zc], -1)
    j1 = torch.stack([zero, -fy / zc, -fy * ty / zc], -1)
    g0 = (j0[:, None, :] @ m)[:, 0]
    g1 = (j1[:, None, :] @ m)[:, 0]
    a = (g0 * g0).sum(-1) + 0.3
    b = (g0 * g1).sum(-1)
    c = (g1 * g1).sum(-1) + 0.3
    det = (a * c - b * b).clamp(min=1e-12)
    conic = torch.stack([c / det, -b / det, a / det], -1)
    opacity = torch.sigmoid(gs.logit_opacity)
    col = gs.radiance()

    with torch.no_grad():
        mid = 0.5 * (a + c)
        radius = 3.0 * torch.sqrt(mid + torch.sqrt((mid * mid - det).clamp(min=0.1)))
        tiles_x, tiles_y = math.ceil(w / TILE), math.ceil(h / TILE)
        ok = (tz > 0.1) & (u + radius > 0) & (u - radius < w) & (v + radius > 0) & (v - radius < h)
        x0 = ((u - radius) / TILE).floor().clamp(0, tiles_x - 1).long()
        x1 = ((u + radius) / TILE).floor().clamp(0, tiles_x - 1).long()
        y0 = ((v - radius) / TILE).floor().clamp(0, tiles_y - 1).long()
        y1 = ((v + radius) / TILE).floor().clamp(0, tiles_y - 1).long()
        span = x1 - x0 + 1
        counts = torch.where(ok, span * (y1 - y0 + 1), torch.zeros_like(span))
        gid = torch.repeat_interleave(torch.arange(len(counts), device=dev), counts)
        first = torch.repeat_interleave(torch.cumsum(counts, 0) - counts, counts)
        within = torch.arange(len(gid), device=dev) - first
        tile = (y0[gid] + within // span[gid]) * tiles_x + x0[gid] + within % span[gid]
        # Front to back within each tile: sort by depth, then stably by tile.
        order = torch.argsort(tz[gid], stable=True)
        gid, tile = gid[order], tile[order]
        order = torch.argsort(tile, stable=True)
        gid, tile = gid[order], tile[order]
        n_tiles = tiles_x * tiles_y
        starts = torch.searchsorted(tile, torch.arange(n_tiles, device=dev))
        pos = torch.arange(len(tile), device=dev) - starts[tile]
        keep = pos < k_max
        STATS["pairs"] += len(pos)
        STATS["dropped"] += int((~keep).sum().item())
        k = int(min(k_max, max(1, int(pos.max().item()) + 1 if len(pos) else 1)))
        idx = torch.full((n_tiles, k), -1, dtype=torch.long, device=dev)
        idx[tile[keep], pos[keep]] = gid[keep]
        ty_, tx_ = torch.meshgrid(torch.arange(tiles_y, device=dev), torch.arange(tiles_x, device=dev), indexing="ij")
        py, px = torch.meshgrid(torch.arange(TILE, device=dev), torch.arange(TILE, device=dev), indexing="ij")
        pix_x = (tx_.reshape(-1, 1) * TILE + px.reshape(1, -1)).float() + 0.5
        pix_y = (ty_.reshape(-1, 1) * TILE + py.reshape(1, -1)).float() + 0.5
        valid = idx >= 0
        gi = idx.clamp(min=0)

    dx = pix_x[:, None, :] - u[gi][:, :, None]
    dy = pix_y[:, None, :] - v[gi][:, :, None]
    cn = conic[gi]
    power = -0.5 * (cn[..., 0, None] * dx * dx + cn[..., 2, None] * dy * dy) - cn[..., 1, None] * dx * dy
    alpha = (opacity[gi][..., None] * torch.exp(power.clamp(max=0.0))).clamp(max=0.99)
    alpha = torch.where(valid[..., None] & (alpha >= 1.0 / 255.0), alpha, torch.zeros_like(alpha))
    trans_incl = torch.cumprod(1.0 - alpha, dim=1)
    trans_excl = torch.cat([torch.ones_like(trans_incl[:, :1]), trans_incl[:, :-1]], dim=1)
    weight = alpha * trans_excl
    out = (weight[..., None] * col[gi][:, :, None, :]).sum(1) + trans_incl[:, -1, :, None] * bg_lin
    img = out.reshape(tiles_y, tiles_x, TILE, TILE, 3).permute(0, 2, 1, 3, 4).reshape(tiles_y * TILE, tiles_x * TILE, 3)
    return agx(img[:h, :w] * exposure)


def psnr(a, b):
    mse = float(((a - b) ** 2).mean())
    return 10 * math.log10(1.0 / max(mse, 1e-12))


def relocate(gs: Gaussians, opt: torch.optim.Adam, gen):
    with torch.no_grad():
        op = torch.sigmoid(gs.logit_opacity)
        dead = (op < 0.005).nonzero().squeeze(1)
        alive = (op >= 0.005).nonzero().squeeze(1)
        if len(dead) == 0 or len(alive) == 0:
            return 0
        pick = alive[torch.multinomial(op[alive].cpu(), len(dead), replacement=True, generator=gen).to(op.device)]
        jitter = torch.randn(len(dead), 3, generator=gen).to(op.device) * gs.log_scales[pick].exp()
        gs.means[dead] = gs.means[pick] + jitter
        gs.log_scales[dead] = gs.log_scales[pick] - math.log(1.6)
        gs.quats[dead] = gs.quats[pick]
        gs.color_raw[dead] = gs.color_raw[pick]
        # Split the opacity so the pair together keeps the original's coverage.
        new_op = 1.0 - torch.sqrt(1.0 - op[pick].clamp(max=0.99))
        gs.logit_opacity[dead] = torch.logit(new_op.clamp(1e-4, 0.99))
        gs.logit_opacity[pick] = torch.logit(new_op.clamp(1e-4, 0.99))
        for p in (gs.means, gs.log_scales, gs.quats, gs.color_raw, gs.logit_opacity):
            st = opt.state.get(p)
            if st:
                st["exp_avg"][dead] = 0
                st["exp_avg_sq"][dead] = 0
        return len(dead)


def write_ply(path: pathlib.Path, gs: Gaussians):
    """The 3DGS training layout (what splat/loader.rs reads), SH degree 0."""
    with torch.no_grad():
        means = gs.means.cpu().numpy()
        lin = gs.radiance().cpu().numpy()
        # Display-encode the linear radiance: the engine decodes it back (srgb_to_linear).
        disp = np.where(lin <= 0.0031308, lin * 12.92, 1.055 * np.power(np.maximum(lin, 0), 1 / 2.4) - 0.055)
        f_dc = (disp - 0.5) / C0
        q = gs.quats / gs.quats.norm(dim=-1, keepdim=True)
        q = q.cpu().numpy()
        rec = np.concatenate([
            means, np.zeros_like(means), f_dc,
            gs.logit_opacity.cpu().numpy()[:, None], gs.log_scales.cpu().numpy(),
            q[:, 3:4], q[:, 0:3],
        ], axis=1).astype("<f4")
    props = ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity",
             "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3"]
    head = "ply\nformat binary_little_endian 1.0\nelement vertex %d\n" % len(rec)
    head += "".join("property float %s\n" % p for p in props) + "end_header\n"
    with open(path, "wb") as f:
        f.write(head.encode())
        f.write(rec.tobytes())


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dataset", type=pathlib.Path)
    ap.add_argument("--out", type=pathlib.Path)
    ap.add_argument("--iters", type=int, default=2500)
    ap.add_argument("--gaussians", type=int, default=30000)
    ap.add_argument("--reg", type=float, default=0.0, help="opacity and scale L1 weight")
    ap.add_argument("--k", type=int, default=1024, help="Gaussians composited per 16x16 tile at most")
    ap.add_argument("--device", default="mps" if torch.backends.mps.is_available() else "cpu")
    ap.add_argument("--score", action="store_true", help="PSNR of the engine's render_NNN.png against view_NNN.png")
    args = ap.parse_args()
    w, h, fov, exposure, sky, views = load_dataset(args.dataset)
    held = set(range(0, len(views), 8))

    if args.score:
        rows = []
        for i, (file, _, _) in enumerate(views):
            r = args.dataset / file.replace("view_", "render_")
            if r.exists():
                rows.append((i in held, psnr(load_image(r), load_image(args.dataset / file))))
        for name, sel in (("held-out", True), ("training", False)):
            v = [p for hld, p in rows if hld == sel]
            if v:
                print(f"engine render vs reference, {name} views: mean PSNR {np.mean(v):.2f} dB over {len(v)}")
        return

    dev = torch.device(args.device)
    images = [torch.from_numpy(load_image(args.dataset / f)).to(dev) for f, _, _ in views]
    cams = []
    for _, eye, target in views:
        r, t = look_at(eye, target)
        cams.append((torch.from_numpy(r).to(dev), torch.from_numpy(t).to(dev)))
    gs = Gaussians(args.gaussians, dev)
    bg = torch.tensor(sky, device=dev)
    opt = torch.optim.Adam([
        {"params": [gs.means], "lr": 8e-4},
        {"params": [gs.log_scales], "lr": 5e-3},
        {"params": [gs.quats], "lr": 1e-3},
        {"params": [gs.logit_opacity], "lr": 5e-2},
        {"params": [gs.color_raw], "lr": 1e-2},
    ], eps=1e-15)
    train = [i for i in range(len(views)) if i not in held]
    gen = torch.Generator().manual_seed(7)
    t0 = time.time()
    for it in range(args.iters):
        frac = it / max(args.iters - 1, 1)
        opt.param_groups[0]["lr"] = 8e-4 * (0.01 ** frac)
        i = train[int(torch.randint(len(train), (1,), generator=gen))]
        img = render(gs, *cams[i], w, h, fov, bg, exposure, args.k)
        loss = (img - images[i]).abs().mean()
        loss = loss + args.reg * (torch.sigmoid(gs.logit_opacity).mean() + gs.log_scales.exp().mean())
        opt.zero_grad(set_to_none=False)
        loss.backward()
        opt.step()
        if it % 300 == 299 and frac < 0.8:
            moved = relocate(gs, opt, gen)
        else:
            moved = None
        if it % 250 == 0 or it == args.iters - 1:
            dropped = STATS["dropped"] / max(STATS["pairs"], 1)
            STATS["pairs"] = STATS["dropped"] = 0
            msg = f"step {it:5d}  L1 {loss.item():.4f}  {time.time() - t0:6.1f} s  dropped {dropped:.1%}"
            if moved is not None:
                msg += f"  relocated {moved}"
            print(msg, flush=True)
    with torch.no_grad():
        scores = [psnr(render(gs, *cams[i], w, h, fov, bg, exposure, args.k).cpu().numpy(), images[i].cpu().numpy()) for i in sorted(held)]
        train_scores = [psnr(render(gs, *cams[i], w, h, fov, bg, exposure, args.k).cpu().numpy(), images[i].cpu().numpy()) for i in train[:8]]
    print(f"fitter's own render: held-out PSNR {np.mean(scores):.2f} dB ({len(scores)} views), "
          f"training PSNR {np.mean(train_scores):.2f} dB; {time.time() - t0:.0f} s on {dev}")
    out = args.out or (args.dataset / "fitted.ply")
    write_ply(out, gs)
    print(f"wrote {out} ({args.gaussians} splats)")


if __name__ == "__main__":
    main()

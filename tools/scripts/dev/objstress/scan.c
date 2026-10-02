// A scanner-style closed surface: a lumpy displaced sphere, v + vn per vertex, f a//a b//b c//c.
// usage: scan <rings> <segments> <out.obj>   triangles = 2 * rings * segments
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

static double rad(double th, double ph) {
    double x = sin(th) * cos(ph), y = cos(th), z = sin(th) * sin(ph);   // smooth in space, not in (th, ph)
    return 0.5 * (1.0 + 0.07 * sin(5 * x + 1) * sin(4 * y) * cos(3 * z) + 0.03 * sin(13 * x + 11 * z)
                  + 0.012 * sin(29 * y + 23 * x) * cos(31 * z) + 0.004 * sin(83 * x + 71 * y + 59 * z));
}
static void pos(double th, double ph, double* p) {
    double r = rad(th, ph);
    p[0] = r * sin(th) * cos(ph); p[1] = r * cos(th); p[2] = r * sin(th) * sin(ph);
}
int main(int argc, char** argv) {
    if (argc < 4) return 2;
    int R = atoi(argv[1]), S = atoi(argv[2]);
    FILE* f = fopen(argv[3], "w");
    static char buf[1 << 22];
    setvbuf(f, buf, _IOFBF, sizeof buf);
    fprintf(f, "# 3D scan export (synthetic): %d rings x %d segments\n# units: metres\no scan\n", R, S);
    const double eps = 1e-5;
    // vertex 1: north pole, then rings 1..R, then south pole
    double p[3];
    pos(0, 0, p); fprintf(f, "v %.6f %.6f %.6f\nvn 0.0000 1.0000 0.0000\n", p[0], p[1], p[2]);
    for (int i = 1; i <= R; ++i) {
        double th = M_PI * i / (R + 1);
        for (int j = 0; j < S; ++j) {
            double ph = 2 * M_PI * j / S, a[3], b[3], c[3], d[3];
            pos(th, ph, p);
            pos(th + eps, ph, a); pos(th - eps, ph, b); pos(th, ph + eps, c); pos(th, ph - eps, d);
            double dt[3] = {a[0] - b[0], a[1] - b[1], a[2] - b[2]}, dp[3] = {c[0] - d[0], c[1] - d[1], c[2] - d[2]};
            double n[3] = {dp[1] * dt[2] - dp[2] * dt[1], dp[2] * dt[0] - dp[0] * dt[2], dp[0] * dt[1] - dp[1] * dt[0]};
            double l = sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
            if (n[0] * p[0] + n[1] * p[1] + n[2] * p[2] < 0) l = -l;
            fprintf(f, "v %.6f %.6f %.6f\nvn %.4f %.4f %.4f\n", p[0], p[1], p[2], n[0] / l, n[1] / l, n[2] / l);
        }
    }
    pos(M_PI, 0, p); fprintf(f, "v %.6f %.6f %.6f\nvn 0.0000 -1.0000 0.0000\n", p[0], p[1], p[2]);
    const long south = 2 + (long)R * S;
    #define V(i, j) (2 + (long)((i) - 1) * S + ((j) % S))
    for (int j = 0; j < S; ++j) fprintf(f, "f 1//1 %ld//%ld %ld//%ld\n", V(1, j + 1), V(1, j + 1), V(1, j), V(1, j));
    for (int i = 1; i < R; ++i)
        for (int j = 0; j < S; ++j) {
            long a = V(i, j), b = V(i, j + 1), c = V(i + 1, j + 1), d = V(i + 1, j);
            fprintf(f, "f %ld//%ld %ld//%ld %ld//%ld\nf %ld//%ld %ld//%ld %ld//%ld\n", a, a, b, b, c, c, a, a, c, c, d, d);
        }
    for (int j = 0; j < S; ++j) fprintf(f, "f %ld//%ld %ld//%ld %ld//%ld\n", south, south, V(R, j), V(R, j), V(R, j + 1), V(R, j + 1));
    fclose(f);
    printf("triangles %ld vertices %ld\n", 2L * R * S, south);
    return 0;
}

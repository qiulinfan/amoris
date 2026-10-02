// What a plain single-threaded OBJ reader costs on the same files: mmap, hand-rolled number parsing,
// fan triangulation, (v,vt,vn) corner dedupe in an open-addressing hash, smooth normals for corners
// without vn, interleaved 64-byte vertices like assets::MeshVertex. No error handling beyond bounds.
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <time.h>
#include <math.h>
typedef struct { float p[3], n[3], uv[2], c[4], t[4]; } Vert;   // 64 bytes
static double now(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return ts.tv_sec + ts.tv_nsec * 1e-9; }
static const char* skip(const char* s) { while (*s == ' ' || *s == '\t') ++s; return s; }
static float num(const char** ps) {   // decimal with optional sign, fraction, exponent
    const char* s = skip(*ps); int neg = 0; double v = 0, f = 0.1;
    if (*s == '-') { neg = 1; ++s; } else if (*s == '+') ++s;
    while (*s >= '0' && *s <= '9') v = v * 10 + (*s++ - '0');
    if (*s == '.') { ++s; while (*s >= '0' && *s <= '9') { v += (*s++ - '0') * f; f *= 0.1; } }
    if (*s == 'e' || *s == 'E') { char* e; long x = strtol(s + 1, &e, 10); v *= pow(10, x); s = e; }
    *ps = s; return (float)(neg ? -v : v);
}
static long inum(const char** ps) { const char* s = *ps; int neg = 0; long v = 0; if (*s == '-') { neg = 1; ++s; } while (*s >= '0' && *s <= '9') v = v * 10 + (*s++ - '0'); *ps = s; return neg ? -v : v; }
#define GROW(a, n, cap) if ((n) >= (cap)) { cap = cap ? cap * 2 : 1024; a = realloc(a, sizeof(*a) * cap); }
int main(int argc, char** argv) {
    double t0 = now();
    int fd = open(argv[1], O_RDONLY); struct stat st; fstat(fd, &st);
    const char* d = mmap(0, st.st_size, PROT_READ, MAP_PRIVATE, fd, 0); const char* end = d + st.st_size;
    float *P = 0, *N = 0; size_t np = 0, nn = 0, cp = 0, cn = 0;
    int64_t* C = 0; size_t nc = 0, cc = 0;   // corners: packed v | n<<32 (no vt in these files)
    for (const char* s = d; s < end;) {
        const char* e = memchr(s, '\n', end - s); if (!e) e = end;
        if (s[0] == 'v' && s[1] == ' ') { GROW(P, np * 3 + 3, cp); const char* q = s + 2; P[np * 3] = num(&q); P[np * 3 + 1] = num(&q); P[np * 3 + 2] = num(&q); ++np; }
        else if (s[0] == 'v' && s[1] == 'n') { GROW(N, nn * 3 + 3, cn); const char* q = s + 3; N[nn * 3] = num(&q); N[nn * 3 + 1] = num(&q); N[nn * 3 + 2] = num(&q); ++nn; }
        else if (s[0] == 'f' && s[1] == ' ') {
            int64_t poly[64]; int k = 0; const char* q = s + 2;
            while (q < e && k < 64) {
                q = skip(q); if (q >= e || *q == '\r') break;
                long v = inum(&q), n = 0; if (v < 0) v += np + 1;
                if (*q == '/') { ++q; if (*q != '/') inum(&q); if (*q == '/') { ++q; n = inum(&q); if (n < 0) n += nn + 1; } }
                poly[k++] = (v - 1) | ((int64_t)(n ? n - 1 : 0x7fffffff) << 32);
                while (q < e && *q != ' ' && *q != '\t') ++q;
            }
            for (int i = 1; i + 1 < k; ++i) { GROW(C, nc + 3, cc); C[nc++] = poly[0]; C[nc++] = poly[i]; C[nc++] = poly[i + 1]; }
        }
        s = e + 1;
    }
    double t1 = now();
    // dedupe corners into vertices
    size_t hcap = 1; while (hcap < nc) hcap <<= 1; hcap <<= 1;
    int64_t* hk = malloc(hcap * 8); uint32_t* hv = malloc(hcap * 4); memset(hk, 0xff, hcap * 8);
    Vert* V = malloc(sizeof(Vert) * (np + nc / 6 + 16)); size_t nv = 0, vcap = np + nc / 6 + 16;
    uint32_t* I = malloc(4 * nc);
    float* SN = calloc(np * 3, 4);
    for (size_t i = 0; i < nc; i += 3) {   // smooth normals per position for corners without vn
        const float *a = P + (C[i] & 0xffffffff) * 3, *b = P + (C[i + 1] & 0xffffffff) * 3, *c = P + (C[i + 2] & 0xffffffff) * 3;
        float u[3] = {b[0] - a[0], b[1] - a[1], b[2] - a[2]}, w[3] = {c[0] - a[0], c[1] - a[1], c[2] - a[2]};
        float f[3] = {u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]};
        for (int j = 0; j < 3; ++j) { float* s = SN + (C[i + j] & 0xffffffff) * 3; s[0] += f[0]; s[1] += f[1]; s[2] += f[2]; }
    }
    for (size_t i = 0; i < nc; ++i) {
        uint64_t key = (uint64_t)C[i], h = (key * 0x9E3779B97F4A7C15ull) & (hcap - 1);
        while (hk[h] != -1 && hk[h] != (int64_t)key) h = (h + 1) & (hcap - 1);
        if (hk[h] == -1) {
            hk[h] = key; if (nv >= vcap) { vcap *= 2; V = realloc(V, sizeof(Vert) * vcap); }
            Vert* v = &V[nv]; memset(v, 0, sizeof *v); size_t pi = key & 0xffffffff, ni = key >> 32;
            memcpy(v->p, P + pi * 3, 12); memcpy(v->n, ni != 0x7fffffff ? N + ni * 3 : SN + pi * 3, 12);
            v->c[0] = v->c[1] = v->c[2] = v->c[3] = 1; hv[h] = (uint32_t)nv++;
        }
        I[i] = hv[h];
    }
    double t2 = now();
    printf("{\"file\": \"%s\", \"mb\": %.1f, \"tokenize_s\": %.3f, \"dedupe_s\": %.3f, \"total_s\": %.3f, \"positions\": %zu, \"vertices\": %zu, \"triangles\": %zu}\n", argv[1], st.st_size / 1e6, t1 - t0, t2 - t1, t2 - t0, np, nv, nc / 3);
    return 0;
}

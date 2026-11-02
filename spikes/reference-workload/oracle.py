"""Independent implementation of the reference workload specification, version 1.

Pure Python, standard library only, written from the specification without
looking at the Rust reference implementation, to cross-check its hashes: the
self-checks of the Rust crate assert the values this script prints. Every f32
operation is computed in double and rounded to binary32 through the IEEE 754
encoding, which is exact for addition, subtraction, multiplication, division
and square root because binary64 carries more than 2 * 24 + 2 significant bits
(double rounding is innocuous there). The wander system is not reproduced: its
sine and cosine come from the platform math library, which this script cannot
imitate.

Usage: python3 oracle.py <entity count> <tick count>
"""
import math
import struct
import sys

M64 = (1 << 64) - 1


def f32(v):
    return struct.unpack("<f", struct.pack("<f", v))[0]


class SplitMix64:
    def __init__(self, seed):
        self.state = seed & M64

    def next(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) & M64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & M64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & M64
        return z ^ (z >> 31)

    def u01(self):
        return f32(float(self.next() >> 40) * 2.0 ** -24)

    def mass(self):
        return [1.0, 2.0, 4.0][self.next() % 3]


W = 1024.0
C = 16.0
R = 8.0
K_STRENGTH = 64.0
VMAX = 64.0
DT = 2.0 ** -6
DAMP = 1.0 - 2.0 ** -8
KILLS = 8
SPAWNS = 8
CHUNK = 256
GRID = int(W / C)


def fnv1a(data, h=0xCBF29CE484222325):
    for b in data:
        h = ((h ^ b) * 0x100000001B3) & M64
    return h


class State:
    def __init__(self, n, seed):
        self.n = n
        self.rng = SplitMix64(seed)
        self.generation = [0] * n
        self.alive = [True] * n
        self.x = [0.0] * n
        self.y = [0.0] * n
        self.vx = [0.0] * n
        self.vy = [0.0] * n
        self.mass = [0.0] * n
        for i in range(n):
            self.draw(i)
        self.energy = 0.0

    def draw(self, i):
        self.x[i] = f32(self.rng.u01() * W)
        self.y[i] = f32(self.rng.u01() * W)
        self.vx[i] = f32(f32(f32(self.rng.u01() * 2.0) - 1.0) * 32.0)
        self.vy[i] = f32(f32(f32(self.rng.u01() * 2.0) - 1.0) * 32.0)
        self.mass[i] = self.rng.mass()

    def hash(self):
        out = bytearray()
        for i in range(self.n):
            out += struct.pack("<I", self.generation[i])
            out += b"\x01" if self.alive[i] else b"\x00"
            if self.alive[i]:
                out += struct.pack("<fffff", self.x[i], self.y[i], self.vx[i], self.vy[i], self.mass[i])
        out += struct.pack("<f", self.energy)
        out += struct.pack("<Q", self.rng.state)
        return fnv1a(out)

    def tick(self, t, wander):
        n = self.n
        # 1. spatial hashing
        cells = [[] for _ in range(GRID * GRID)]
        cell_of = [0] * n
        for i in range(n):
            if self.alive[i]:
                cx = int(math.floor(f32(self.x[i] / C)))
                cy = int(math.floor(f32(self.y[i] / C)))
                cell_of[i] = (cx, cy)
                cells[cy * GRID + cx].append(i)
        # 2. repulsion
        new_vx = list(self.vx)
        new_vy = list(self.vy)
        for i in range(n):
            if not self.alive[i]:
                continue
            xi, yi, mi = self.x[i], self.y[i], self.mass[i]
            cx, cy = cell_of[i]
            fx = 0.0
            fy = 0.0
            for b in (-1, 0, 1):
                for a in (-1, 0, 1):
                    gx, gy = cx + a, cy + b
                    if gx < 0 or gy < 0 or gx >= GRID or gy >= GRID:
                        continue
                    for j in cells[gy * GRID + gx]:
                        if j == i:
                            continue
                        dx = f32(self.x[j] - xi)
                        dy = f32(self.y[j] - yi)
                        d2 = f32(f32(dx * dx) + f32(dy * dy))
                        if d2 < R * R and d2 > 0.0:
                            d = f32(math.sqrt(d2))
                            s = f32(f32(K_STRENGTH * f32(R - d)) / d)
                            fx = f32(fx - f32(dx * s))
                            fy = f32(fy - f32(dy * s))
            ax = f32(fx / mi)
            ay = f32(fy / mi)
            new_vx[i] = clamp(f32(f32(self.vx[i] + f32(ax * DT)) * DAMP))
            new_vy[i] = clamp(f32(f32(self.vy[i] + f32(ay * DT)) * DAMP))
        self.vx = new_vx
        self.vy = new_vy
        # 3. wander (off in the oracle: platform sin and cos are not reproduced here)
        assert not wander
        # 4. integration and 5. boundary
        for i in range(n):
            if not self.alive[i]:
                continue
            self.x[i] = f32(self.x[i] + f32(self.vx[i] * DT))
            self.y[i] = f32(self.y[i] + f32(self.vy[i] * DT))
            self.x[i], self.vx[i] = bounce(self.x[i], self.vx[i])
            self.y[i], self.vy[i] = bounce(self.y[i], self.vy[i])
        # 6. energy
        energy = 0.0
        for c in range((n + CHUNK - 1) // CHUNK):
            chunk_sum = 0.0
            for i in range(c * CHUNK, min((c + 1) * CHUNK, n)):
                if self.alive[i]:
                    e = f32(f32(f32(self.vx[i] * self.vx[i]) + f32(self.vy[i] * self.vy[i])) * f32(0.5 * self.mass[i]))
                    chunk_sum = f32(chunk_sum + e)
            energy = f32(energy + chunk_sum)
        self.energy = energy
        # 7. structural changes
        for _ in range(KILLS):
            j = self.rng.next() % n
            self.alive[j] = False
        for _ in range(SPAWNS):
            dead = next((i for i in range(n) if not self.alive[i]), None)
            if dead is None:
                break
            self.draw(dead)
            self.generation[dead] += 1
            self.alive[dead] = True


def clamp(v):
    if v < -VMAX:
        return -VMAX
    if v > VMAX:
        return VMAX
    return v


def bounce(p, v):
    if p < 0.0:
        p = -p
        v = -v
    if p >= W:
        p = f32(2.0 * W - p)
        v = -v
    if p >= W:
        p = W - 0.0625
    return p, v


def run(n, ticks, seed=0x4C7564656E656121):
    state = State(n, seed)
    hashes = [state.hash()]
    for t in range(1, ticks + 1):
        state.tick(t, False)
        hashes.append(state.hash())
    run_hash = fnv1a(b"".join(struct.pack("<Q", h) for h in hashes))
    return hashes, run_hash


if __name__ == "__main__":
    n = int(sys.argv[1])
    ticks = int(sys.argv[2])
    hashes, run_hash = run(n, ticks)
    for t, h in enumerate(hashes):
        print(f"h_{t} = 0x{h:016x}")
    print(f"H = 0x{run_hash:016x}")

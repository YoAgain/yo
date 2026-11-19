"""A QR code as SVG (ISO/IEC 18004), standard library only: byte mode, error correction M, versions 1–10 (up to 213
bytes), enough for a link. Drawn on the server, so the page loads nothing from elsewhere."""

# Per version 1–10 at level M: error correction codewords per block, and blocks.
ECC_PER_BLOCK = [10, 16, 26, 18, 24, 16, 18, 22, 22, 26]
BLOCKS = [1, 1, 1, 2, 2, 4, 4, 4, 5, 5]
ALIGNMENT = [[], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50]]
FORMAT_M = 0  # the two format bits for level M


def _mul(x, y):
    z = 0
    for i in reversed(range(8)):
        z = (z << 1) ^ ((z >> 7) * 0x11D)
        z ^= ((y >> i) & 1) * x
    return z


def _rs_divisor(degree):
    result = [0] * (degree - 1) + [1]
    root = 1
    for _ in range(degree):
        for j in range(degree):
            result[j] = _mul(result[j], root)
            if j + 1 < degree:
                result[j] ^= result[j + 1]
        root = _mul(root, 0x02)
    return result


def _rs_remainder(data, divisor):
    result = [0] * len(divisor)
    for b in data:
        factor = b ^ result.pop(0)
        result.append(0)
        for i, coef in enumerate(divisor):
            result[i] ^= _mul(coef, factor)
    return result


def _raw_modules(ver):
    result = (16 * ver + 128) * ver + 64
    if ver >= 2:
        align = ver // 7 + 2
        result -= (25 * align - 10) * align - 55
        if ver >= 7:
            result -= 36
    return result


def _data_capacity(ver):
    return _raw_modules(ver) // 8 - ECC_PER_BLOCK[ver - 1] * BLOCKS[ver - 1]


def _codewords(data, ver):
    bits = []

    def put(value, n):
        bits.extend((value >> i) & 1 for i in reversed(range(n)))

    put(0b0100, 4)
    put(len(data), 8 if ver <= 9 else 16)
    for b in data:
        put(b, 8)
    capacity = _data_capacity(ver) * 8
    put(0, min(4, capacity - len(bits)))
    put(0, -len(bits) % 8)
    words = [int("".join(map(str, bits[i : i + 8])), 2) for i in range(0, len(bits), 8)]
    pad = 0xEC
    while len(words) < capacity // 8:
        words.append(pad)
        pad ^= 0xEC ^ 0x11
    # Split into blocks, add error correction, interleave.
    blocks_n, ecc_n = BLOCKS[ver - 1], ECC_PER_BLOCK[ver - 1]
    raw = _raw_modules(ver) // 8
    short_n = blocks_n - raw % blocks_n
    short_len = raw // blocks_n
    divisor = _rs_divisor(ecc_n)
    blocks, k = [], 0
    for i in range(blocks_n):
        dat = words[k : k + short_len - ecc_n + (0 if i < short_n else 1)]
        k += len(dat)
        ecc = _rs_remainder(dat, divisor)
        if i < short_n:
            dat = dat + [0]
        blocks.append(dat + ecc)
    out = []
    for i in range(len(blocks[0])):
        for j, block in enumerate(blocks):
            if i != short_len - ecc_n or j >= short_n:
                out.append(block[i])
    return out


class _Grid:
    def __init__(self, ver):
        self.ver, self.size = ver, ver * 4 + 17
        self.dark = [[False] * self.size for _ in range(self.size)]
        self.fixed = [[False] * self.size for _ in range(self.size)]

    def set(self, x, y, dark):
        self.dark[y][x] = bool(dark)
        self.fixed[y][x] = True

    def functions(self):
        size = self.size
        for i in range(size):
            self.set(6, i, i % 2 == 0)
            self.set(i, 6, i % 2 == 0)
        for cx, cy in ((3, 3), (size - 4, 3), (3, size - 4)):
            for dy in range(-4, 5):
                for dx in range(-4, 5):
                    x, y = cx + dx, cy + dy
                    if 0 <= x < size and 0 <= y < size:
                        d = max(abs(dx), abs(dy))
                        self.set(x, y, d not in (2, 4))
        pos = ALIGNMENT[self.ver - 1]
        last = len(pos) - 1
        for i, ax in enumerate(pos):
            for j, ay in enumerate(pos):
                if (i, j) in ((0, 0), (0, last), (last, 0)):
                    continue
                for dy in range(-2, 3):
                    for dx in range(-2, 3):
                        self.set(ax + dx, ay + dy, max(abs(dx), abs(dy)) != 1)
        self.format_bits(0)
        if self.ver >= 7:
            rem = self.ver
            for _ in range(12):
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25)
            bits = self.ver << 12 | rem
            for i in range(18):
                bit = (bits >> i) & 1
                a, b = size - 11 + i % 3, i // 3
                self.set(a, b, bit)
                self.set(b, a, bit)

    def format_bits(self, mask):
        data = FORMAT_M << 3 | mask
        rem = data
        for _ in range(10):
            rem = (rem << 1) ^ ((rem >> 9) * 0x537)
        bits = (data << 10 | rem) ^ 0x5412
        bit = lambda i: (bits >> i) & 1  # noqa: E731
        size = self.size
        for i in range(6):
            self.set(8, i, bit(i))
        self.set(8, 7, bit(6))
        self.set(8, 8, bit(7))
        self.set(7, 8, bit(8))
        for i in range(9, 15):
            self.set(14 - i, 8, bit(i))
        for i in range(8):
            self.set(size - 1 - i, 8, bit(i))
        for i in range(8, 15):
            self.set(8, size - 15 + i, bit(i))
        self.set(8, size - 8, True)

    def place(self, words):
        size, i, right = self.size, 0, self.size - 1
        while right >= 1:
            if right == 6:
                right = 5
            for vert in range(size):
                for j in range(2):
                    x = right - j
                    y = size - 1 - vert if ((right + 1) & 2) == 0 else vert
                    if not self.fixed[y][x] and i < len(words) * 8:
                        self.dark[y][x] = bool((words[i >> 3] >> (7 - (i & 7))) & 1)
                        i += 1
            right -= 2

    def apply(self, mask):
        test = [
            lambda x, y: (x + y) % 2 == 0,
            lambda x, y: y % 2 == 0,
            lambda x, y: x % 3 == 0,
            lambda x, y: (x + y) % 3 == 0,
            lambda x, y: (x // 3 + y // 2) % 2 == 0,
            lambda x, y: x * y % 2 + x * y % 3 == 0,
            lambda x, y: (x * y % 2 + x * y % 3) % 2 == 0,
            lambda x, y: ((x + y) % 2 + x * y % 3) % 2 == 0,
        ][mask]
        for y in range(self.size):
            for x in range(self.size):
                if not self.fixed[y][x] and test(x, y):
                    self.dark[y][x] = not self.dark[y][x]

    def penalty(self):
        """Runs, 2×2 blocks and balance (the standard's rules 1, 2 and 4)."""
        size, score = self.size, 0
        for lines in (self.dark, list(zip(*self.dark))):
            for line in lines:
                run = 1
                for a, b in zip(line, line[1:]):
                    if a == b:
                        run += 1
                    else:
                        score += run - 2 if run >= 5 else 0
                        run = 1
                score += run - 2 if run >= 5 else 0
        for y in range(size - 1):
            for x in range(size - 1):
                c = self.dark[y][x]
                if c == self.dark[y][x + 1] == self.dark[y + 1][x] == self.dark[y + 1][x + 1]:
                    score += 3
        dark = sum(map(sum, self.dark))
        score += (abs(dark * 20 - size * size * 10) + size * size - 1) // (size * size) * 10
        return score


def matrix(text):
    """The modules (rows of booleans, True dark) for `text`, without the quiet zone."""
    data = text.encode()
    ver = next((v for v in range(1, 11) if len(data) + 3 <= _data_capacity(v) + (1 if v <= 9 else 0)), None)
    if ver is None:
        raise ValueError("too long for a QR code here")
    words = _codewords(data, ver)
    best = None
    for mask in range(8):
        grid = _Grid(ver)
        grid.functions()
        grid.place(words)
        grid.apply(mask)
        grid.format_bits(mask)
        score = grid.penalty()
        if best is None or score < best[0]:
            best = (score, grid)
    return best[1].dark


def svg(text, quiet=4):
    """`text` as a QR code in SVG, one path, `currentColor` on transparent, with its quiet zone."""
    rows = matrix(text)
    n = len(rows) + 2 * quiet
    path = "".join(
        f"M{x + quiet},{y + quiet}h1v1h-1z" for y, row in enumerate(rows) for x, dark in enumerate(row) if dark
    )
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" shape-rendering="crispEdges">'
        f'<path fill="currentColor" d="{path}"/></svg>'
    )

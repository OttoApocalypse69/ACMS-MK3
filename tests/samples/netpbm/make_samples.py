"""Writes the Netpbm sample images with Pillow, an implementation independent of ACMS, and a
PNG of each that says what ACMS must read from it. Run from this folder:

    python make_samples.py
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent

# 13 x 5: odd widths make PBM rows end in the middle of a byte.
W, H = 13, 5


def colour(x: int, y: int) -> tuple[int, int, int]:
    return ((x * 19) % 256, (y * 61) % 256, ((x + y) * 37) % 256)


def main() -> None:
    rgb = Image.new("RGB", (W, H))
    grey = Image.new("L", (W, H))
    bits = Image.new("1", (W, H))
    for y in range(H):
        for x in range(W):
            rgb.putpixel((x, y), colour(x, y))
            grey.putpixel((x, y), (x * 20 + y * 3) % 256)
            # A pattern that is not symmetric, so mistakes in bit order show.
            bits.putpixel((x, y), 255 if (x * x + y) % 3 else 0)

    rgb.save(HERE / "colour.ppm")
    rgb.save(HERE / "colour.png")
    grey.save(HERE / "grey.pgm")
    grey.save(HERE / "grey.png")
    bits.save(HERE / "bits.pbm")
    bits.convert("RGB").save(HERE / "bits.png")

    # 16 bits a sample: each value v as v * 257, which reads back as exactly v.
    wide = Image.new("I;16", (W, H))
    for y in range(H):
        for x in range(W):
            wide.putpixel((x, y), grey.getpixel((x, y)) * 257)
    wide.save(HERE / "grey16.pgm")

    for name in ["colour.ppm", "grey.pgm", "bits.pbm", "grey16.pgm"]:
        head = (HERE / name).read_bytes()[:20]
        print(name, head)


if __name__ == "__main__":
    main()

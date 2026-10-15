"""Draws the ACMS icon and writes it in every size the app and its installers need.

The icon is pixel art: it is drawn once at 16 x 16 and every larger size is the same picture
enlarged by a whole number, so it stays crisp. Run from this folder:

    python make_icon.py

It writes `acms.png` (the 16 x 16 original), `acms-<size>.png` for the sizes below, and
`acms.ico` with all of them for Windows.
"""

from pathlib import Path

from PIL import Image

HERE = Path(__file__).resolve().parent
SIZES = [16, 24, 32, 48, 64, 128, 256, 512]

# The picture, one character per pixel. A dark tile with rounded corners; on it a pencil
# drawing a row of coloured pixels.
ART = [
    "..OOOOOOOOOOOO..",
    ".OBBBBBBBBBBPPO.",
    "OBBBBBBBBBBPPPPO",
    "OBBBBBBBBBGPPPPO",
    "OBBBBBBBBYGGPPBO",
    "OBBBBBBBYYyGGBBO",
    "OBBBBBBYYyyyBBBO",
    "OBBBBBYYyyyBBBBO",
    "OBBBBYYyyyBBBBBO",
    "OBBBWWyyyBBBBBBO",
    "OBBBWWWyBBBBBBBO",
    "OBBTTWBBBBBBBBBO",
    "OBBTTBBBBBBBBBBO",
    "OBRRUUNNBBBBBBBO",
    ".ORRUUNNBBBBBBO.",
    "..OOOOOOOOOOOO..",
]

COLORS = {
    ".": (0, 0, 0, 0),
    "O": (0x14, 0x15, 0x1A, 255),  # outline
    "B": (0x26, 0x2A, 0x3A, 255),  # tile
    "P": (0xE8, 0x6A, 0x8A, 255),  # eraser
    "G": (0xA8, 0xAE, 0xBC, 255),  # metal band
    "Y": (0xF6, 0xC9, 0x4E, 255),  # pencil, lit side
    "y": (0xD0, 0x96, 0x2C, 255),  # pencil, shaded side
    "W": (0xE9, 0xCF, 0xA6, 255),  # wood
    "T": (0x3A, 0x3D, 0x48, 255),  # graphite
    "R": (0xE5, 0x53, 0x4B, 255),  # the pixels drawn: red,
    "U": (0x5B, 0x8D, 0xEF, 255),  # the app's accent blue,
    "N": (0x5F, 0xC2, 0x6B, 255),  # green
}


def original() -> Image.Image:
    assert len(ART) == 16 and all(len(row) == 16 for row in ART), "the art must be 16 x 16"
    image = Image.new("RGBA", (16, 16))
    for y, row in enumerate(ART):
        for x, char in enumerate(row):
            image.putpixel((x, y), COLORS[char])
    return image


def main() -> None:
    small = original()
    small.save(HERE / "acms.png")
    sized = {}
    for size in SIZES:
        sized[size] = small.resize((size, size), Image.Resampling.NEAREST)
        sized[size].save(HERE / f"acms-{size}.png")
    # Windows picks the size it needs from the .ico; 256 is the largest it uses.
    ico_sizes = [size for size in SIZES if size <= 256]
    sized[256].save(HERE / "acms.ico", sizes=[(s, s) for s in ico_sizes])
    print("wrote", ", ".join(f"{s}" for s in SIZES), "and acms.ico")


if __name__ == "__main__":
    main()

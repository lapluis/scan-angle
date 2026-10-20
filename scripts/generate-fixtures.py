"""Generate synthetic regression images; never rewrite checked-in golden JSON.

Test-only dependency: Pillow 12.3.0. The application remains pure Rust.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path

import PIL
from PIL import Image, ImageDraw, ImageOps


def borders(root):
    base = Image.new("RGB", (1400, 1700), "#dfeefb")
    ImageDraw.Draw(base).rectangle((140, 170, 1250, 1510), fill="white", outline="#666666", width=3)
    for angle in [-7, -2, -0.25, 0, 0.3, 1.75, 6]:
        base.rotate(-angle, resample=Image.Resampling.BICUBIC, fillcolor="#dfeefb").save(root / f"angle_{angle}.png")
    for circle in [False, True]:
        im = Image.new("RGB", (1200, 1200), "#dfeefb")
        if circle:
            ImageDraw.Draw(im).ellipse((150, 150, 1050, 1050), fill="gold", outline="black", width=5)
        im.save(root / ("circle.png" if circle else "blank.png"))
    canvas = Image.new("RGB", (2000, 2000), "white")
    draw = ImageDraw.Draw(canvas)
    for start, end in [((400, 400), (1600, 400)), ((1600, 400), (1600, 1600)),
                       ((1600, 1600), (400, 1600)), ((400, 1600), (400, 400))]:
        x, y = start
        dx, dy = end[0] - x, end[1] - y
        length = math.hypot(dx, dy)
        offset = 0.0
        while offset < length:
            stop = min(offset + 100, length)
            draw.line([(x + dx / length * offset, y + dy / length * offset),
                       (x + dx / length * stop, y + dy / length * stop)], fill="black", width=3)
            offset = stop + 5
    for angle in [-1.75, 0.3]:
        canvas.rotate(-angle, resample=Image.Resampling.BICUBIC, fillcolor="white").crop((200, 200, 1800, 1800)).save(root / f"fragmented_{angle}.png")


def formats(root):
    root.mkdir(exist_ok=True)
    im = Image.new("RGB", (64, 48), "white")
    draw = ImageDraw.Draw(im)
    for box, color in [((0, 0, 31, 23), "red"), ((32, 0, 63, 23), "green"), ((0, 24, 31, 47), "blue")]:
        draw.rectangle(box, fill=color)
    flipped = ImageOps.mirror(im)
    for extension in ["png", "bmp", "jpg", "webp"]:
        im.save(root / f"color.{extension}")
    for orientation in range(1, 9):
        exif = Image.Exif()
        exif[274] = orientation
        im.save(root / f"exif_{orientation}.jpg", exif=exif, quality=95)
    for mode in ["1", "L", "I", "I;16", "F", "P", "RGBA", "CMYK"]:
        page = im.convert(mode)
        page.save(root / f"tiff_{mode.replace(';', '_')}.tif", save_all=True, append_images=[page.copy()])
    for compression in ["group3", "group4", "jpeg", "packbits", "tiff_adobe_deflate", "tiff_lzw"]:
        page = im.convert("1") if compression.startswith("group") else im
        page.save(root / f"compression_{compression}.tif", compression=compression)
    im.save(root / "animated.png", save_all=True, append_images=[flipped], duration=100, loop=0)
    im.save(root / "default_frame.png", save_all=True, append_images=[flipped], default_image=True, duration=100, loop=0)
    im.save(root / "animated.webp", save_all=True, append_images=[flipped], duration=100, loop=0, lossless=True)
    im.save(root / "multipicture.jpg", format="MPO", save_all=True, append_images=[flipped])


def resize(root):
    root.mkdir(exist_ok=True)
    for width, height in [(517, 333), (2049, 1537)]:
        # A deterministic ramp/checkerboard with high-frequency edges and wraparound.
        im = Image.new("RGB", (width, height))
        im.putdata([((7*x + 3*y) % 256, (11*y + 17*(x//3)) % 256,
                     255 if ((x//9 + y//7) % 2) else 0)
                    for y in range(height) for x in range(width)])
        im.save(root / f"{width}x{height}.png")
        for name in ["lanczos", "bicubic", "bilinear", "nearest", "box", "hamming"]:
            reference = im.copy()
            reference.thumbnail((256, 256), getattr(Image.Resampling, name.upper()))
            reference.save(root / f"{width}_{name}.png")


def verify_formats(root, manifest):
    for case in json.loads(manifest.read_text(encoding="utf-8")):
        with Image.open(root / "formats" / case["file"]) as image:
            assert getattr(image, "n_frames", 1) == len(case["pages"]), case["file"]
            for page, expected in enumerate(case["pages"]):
                image.seek(page)
                oriented = ImageOps.exif_transpose(image).convert("RGB")
                w, h = oriented.size
                assert [w, h] == [expected["width"], expected["height"]], case["file"]
                for point, rgb in zip([(10, 10), (w-10, 10), (10, h-10), (w-10, h-10)], expected["samples"]):
                    assert all(abs(a-b) <= 3 for a,b in zip(oriented.getpixel(point), rgb)), (case["file"], point, oriented.getpixel(point), rgb)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parents[1] / "tests/fixtures")
    args = parser.parse_args()
    if PIL.__version__ != "12.3.0":
        raise SystemExit("Install the pinned test generator dependency: python -m pip install -r scripts/fixture-requirements.txt")
    args.output.mkdir(parents=True, exist_ok=True)
    borders(args.output)
    formats(args.output / "formats")
    resize(args.output / "resize")
    verify_formats(args.output, Path(__file__).resolve().parents[1] / "tests/fixtures/formats/manifest.json")
    images = sorted(p for p in args.output.rglob("*") if p.suffix in {".png", ".jpg", ".bmp", ".webp", ".tif"})
    assert len(images) == 55, len(images)
    # Log deterministic hashes for repeatability checks, without changing reference data.
    hashes = {p.relative_to(args.output).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest() for p in images}
    expected = json.loads((Path(__file__).resolve().parents[1] / "tests/fixtures/image-sha256.json").read_text(encoding="utf-8"))
    if hashes != expected:
        changed = sorted(k for k in hashes.keys() | expected.keys() if hashes.get(k) != expected.get(k))
        raise SystemExit(f"Generated fixtures differ from the pinned corpus: {changed}. Do not update goldens automatically.")
    print(f"Generated and verified {len(images)} fixture images in {args.output}")


if __name__ == "__main__":
    main()
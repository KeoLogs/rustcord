"""Generates background.jpg for the Frosted Glass test theme.

An original, procedurally drawn abstract image (soft purple/indigo/pink
light blobs, heavily blurred) so the theme ships no third-party artwork.
Run: python3 make_background.py   (needs Pillow)
"""
import random
from PIL import Image, ImageDraw, ImageFilter

W, H = 1600, 1000
random.seed(7)
img = Image.new("RGB", (W, H), (14, 12, 34))
d = ImageDraw.Draw(img)
blobs = [
    ((103, 58, 183), 9), ((63, 81, 181), 8), ((255, 51, 159), 3),
    ((0, 176, 244), 1), ((40, 30, 90), 6),
]
for colour, n in blobs:
    for _ in range(n):
        r = random.randint(120, 380)
        x, y = random.randint(-100, W + 100), random.randint(-100, H + 100)
        d.ellipse((x - r, y - r, x + r, y + r), fill=colour)
img = img.filter(ImageFilter.GaussianBlur(90))
# Gentle diagonal light streak for depth.
streak = Image.new("L", (W, H), 0)
ImageDraw.Draw(streak).polygon([(0, H * 0.75), (W * 0.55, 0), (W * 0.7, 0), (0, H)], fill=40)
streak = streak.filter(ImageFilter.GaussianBlur(120))
img = Image.composite(Image.new("RGB", (W, H), (255, 255, 255)), img, streak)
img.save("background.jpg", quality=82, optimize=True, progressive=True)

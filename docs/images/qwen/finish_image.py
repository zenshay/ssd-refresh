"""Finish a raw Qwen-Image PNG for the README.

Use: uv run --with pillow --with numpy python finish_image.py RAW.png STEP_NUMBER OUT.png

What it does:
1. Makes the near-white background pure white.
2. Draws a round badge with the step number at the top left.
3. Adds a thin gray border with round corners. Corners are transparent.
   The image then looks like a white card on the dark GitHub theme.
4. Reduces the PNG to 256 colors.
"""
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

BADGE_COLOR = (108, 99, 201)  # violet-blue from the palette
BORDER_COLOR = (208, 215, 222)
RADIUS = 28


def whiten(img):
    a = np.asarray(img.convert("RGB")).astype(float)
    low, spread = a.min(axis=2), a.max(axis=2) - a.min(axis=2)
    # weight is 1 for near-white gray pixels and 0 for colored pixels
    weight = np.clip((low - 236) / 10, 0, 1) * (spread <= 8)
    a += (255 - a) * weight[..., None]
    return Image.fromarray(a.round().astype("uint8"))


def add_badge(img, number):
    draw = ImageDraw.Draw(img)
    cx, cy, r = 58, 58, 34
    draw.ellipse((cx - r, cy - r, cx + r, cy + r), fill=BADGE_COLOR)
    font = ImageFont.truetype("C:/Windows/Fonts/segoeprb.ttf", 44)
    draw.text((cx, cy), str(number), font=font, fill="white", anchor="mm")


def round_card(img):
    mask = Image.new("L", img.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, *img.size), RADIUS, fill=255)
    out = img.convert("RGBA")
    out.putalpha(mask)
    ImageDraw.Draw(out).rounded_rectangle(
        (0, 0, img.size[0] - 1, img.size[1] - 1), RADIUS, outline=BORDER_COLOR + (255,), width=2
    )
    return out


raw, number, dest = sys.argv[1], sys.argv[2], sys.argv[3]
img = whiten(Image.open(raw))
add_badge(img, number)
round_card(img).quantize(256, method=Image.Quantize.FASTOCTREE).save(dest, optimize=True)

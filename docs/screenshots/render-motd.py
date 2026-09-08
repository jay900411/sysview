#!/usr/bin/env python3
"""把 `sysview --banner` 的輸出畫成 PNG（README 的 docs/screenshots/motd.png）。

用法：python3 docs/screenshots/render-motd.py [sysview 執行檔] [輸出 png]
需要 Pillow 與 DejaVu Sans Mono（Debian/Ubuntu：fonts-dejavu-core）。

解析 SGR：38;5;N 前景、1 粗體、0 重設。字型 DejaVu Sans Mono，方塊字元
以整格繪製，格子大小從字型度量取得，所以方塊之間不會有縫。
"""
import re
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

BIN = sys.argv[1] if len(sys.argv) > 1 else "./target/release/sysview"
OUT = sys.argv[2] if len(sys.argv) > 2 else "docs/screenshots/motd.png"
SIZE = 15
REG = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", SIZE)
BOLD = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf", SIZE)
BG = (28, 28, 28)
FG = (220, 220, 220)
XTERM = {75: (95, 175, 255), 214: (255, 175, 0), 245: (138, 138, 138)}

text = subprocess.run([BIN, "--banner"], check=True, capture_output=True, text=True).stdout
lines = text.split("\n")
if lines and lines[-1] == "":
    lines.pop()

# 格子：寬 = 一個字的 advance，高 = ascent + descent
cw = round(REG.getlength("█"))
asc, desc = REG.getmetrics()
ch = asc + desc

# 先解析成 (char, colour, bold) 的格子
rows = []
sgr = re.compile(r"\x1b\[([0-9;]*)m")
for line in lines:
    cells = []
    fg, bold = FG, False
    i = 0
    while i < len(line):
        m = sgr.match(line, i)
        if m:
            params = [int(p) for p in m.group(1).split(";")] if m.group(1) else [0]
            j = 0
            while j < len(params):
                p = params[j]
                if p == 0:
                    fg, bold = FG, False
                elif p == 1:
                    bold = True
                elif p == 38 and j + 2 < len(params) and params[j + 1] == 5:
                    fg = XTERM.get(params[j + 2], FG)
                    j += 2
                j += 1
            i = m.end()
            continue
        cells.append((line[i], fg, bold))
        i += 1
    rows.append(cells)

cols = max((len(r) for r in rows), default=0)
pad_x, pad_y = cw * 1, ch // 2
img = Image.new("RGB", (cols * cw + 2 * pad_x, len(rows) * ch + 2 * pad_y), BG)
d = ImageDraw.Draw(img)
for y, cells in enumerate(rows):
    for x, (c, fg, bold) in enumerate(cells):
        if c == " ":
            continue
        font = BOLD if bold else REG
        px, py = pad_x + x * cw, pad_y + y * ch
        if "▀" <= c <= "▟":
            # 方塊字元：用字型畫會有縫，直接填矩形（上下左右四個象限）
            q = {
                "▀": (1, 1, 0, 0), "▄": (0, 0, 1, 1), "█": (1, 1, 1, 1),
                "▌": (1, 0, 1, 0), "▐": (0, 1, 0, 1),
                "▘": (1, 0, 0, 0), "▝": (0, 1, 0, 0), "▖": (0, 0, 1, 0), "▗": (0, 0, 0, 1),
                "▙": (1, 0, 1, 1), "▟": (0, 1, 1, 1), "▛": (1, 1, 1, 0), "▜": (1, 1, 0, 1),
                "▚": (1, 0, 0, 1), "▞": (0, 1, 1, 0),
            }.get(c)
            if q is None:
                d.text((px, py), c, font=font, fill=fg)
                continue
            hw, hh = cw / 2, ch / 2
            boxes = [
                (px, py, px + hw, py + hh),                    # 左上
                (px + hw, py, px + cw, py + hh),               # 右上
                (px, py + hh, px + hw, py + ch),               # 左下
                (px + hw, py + hh, px + cw, py + ch),          # 右下
            ]
            for on, b in zip(q, boxes):
                if on:
                    d.rectangle((b[0], b[1], b[2] - 1, b[3] - 1), fill=fg)
        else:
            d.text((px, py), c, font=font, fill=fg)

img = img.resize((img.width * 2, img.height * 2), Image.NEAREST) if SIZE < 12 else img
img.save(OUT)
print(f"{OUT}: {img.width}x{img.height}, {cols} cols x {len(rows)} rows, cell {cw}x{ch}")

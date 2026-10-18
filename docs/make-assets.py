#!/usr/bin/env python3
"""Regenera los gráficos del README a partir de los logos de gui/assets/logos.

Uso:  python3 docs/make-assets.py            (escribe docs/services.svg, how-it-works.svg y social-preview.svg)
      rsvg-convert -w 1280 docs/social-preview.svg -o docs/social-preview.png   (la imagen para GitHub)

Solo usa la biblioteca estándar. Si añades un servicio, añádelo a SERVICES.
El banner (docs/banner.svg) se edita a mano: de él solo se reutilizan la paleta y la onda.
"""
import re
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOGOS = ROOT / "gui" / "assets" / "logos"
DOCS = ROOT / "docs"

FONT = "Inter, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif"

# (nombre, archivo del logo, qué da). Las dos listas se dibujan en filas distintas.
DIRECT = [
    ("YouTube", "youtube.svg", "Vídeo y audio"),
    ("TikTok", "tiktok.svg", "Vídeo sin marca de agua"),
    ("Twitch", "twitch.svg", "Clips"),
    ("Bandcamp", "bandcamp.svg", "Canciones en MP3"),
]
CATALOGS = [
    ("Spotify", "spotify.svg", "Canción, álbum, playlist"),
    ("Deezer", "deezer.svg", "Canción, álbum, playlist"),
    ("Apple Music", "apple-music.svg", "Canción y álbum"),
    ("Tidal", "tidal.svg", "Canción"),
]


def logo(file, x, y, size):
    """Un logo de Simple Icons (24x24, color en el atributo fill de la raíz) en la posición dada."""
    svg = (LOGOS / file).read_text(encoding="utf8")
    fill = re.search(r'<svg[^>]*\sfill="([^"]+)"', svg).group(1)
    inner = re.sub(r"<title>.*?</title>", "", svg.split(">", 1)[1].rsplit("</svg>", 1)[0], flags=re.S)
    return f'<g transform="translate({x} {y}) scale({size / 24:.4f})" fill="{fill}">{inner}</g>'


def defs():
    return (
        '<defs>'
        '<linearGradient id="bg" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#15151b"/><stop offset="0.55" stop-color="#1b1622"/><stop offset="1" stop-color="#241a2f"/></linearGradient>'
        '<linearGradient id="wave" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#ff5a4f"/><stop offset="0.5" stop-color="#ff4d9a"/><stop offset="1" stop-color="#8b5cf6"/></linearGradient>'
        '<linearGradient id="title" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#ffffff"/><stop offset="1" stop-color="#ffd9d5"/></linearGradient>'
        '<radialGradient id="glow" cx="0.5" cy="0.5" r="0.5"><stop offset="0" stop-color="#ff5a4f" stop-opacity="0.22"/><stop offset="1" stop-color="#ff5a4f" stop-opacity="0"/></radialGradient>'
        '<marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M1 1L9 5L1 9" fill="none" stroke="#ff4d9a" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"/></marker>'
        '</defs>'
    )


def text(x, y, s, size, weight=500, fill="#e9e6f2", anchor="start", opacity=1):
    op = f' fill-opacity="{opacity}"' if opacity != 1 else ""
    return f'<text x="{x}" y="{y}" text-anchor="{anchor}" font-family="{FONT}" font-size="{size}" font-weight="{weight}" fill="{fill}"{op}>{escape(s)}</text>'


def card(x, y, w, h, name, file, caption):
    return (
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="18" fill="#ffffff" fill-opacity="0.05" stroke="#ffffff" stroke-opacity="0.14"/>'
        + logo(file, x + 24, y + (h - 40) / 2, 40)
        + text(x + 82, y + h / 2 - 2, name, 21, 700, "#ffffff")
        + text(x + 82, y + h / 2 + 22, caption, 14, 500, "#c9c3d9")
    )


def services_svg():
    w, cw, gap, x0 = 1200, 270, 20, 40
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} 372" width="{w}" height="372" role="img" aria-labelledby="t d">',
           '<title id="t">Servicios compatibles</title>',
           '<desc id="d">YouTube, TikTok, Twitch y Bandcamp se descargan directamente. Spotify, Deezer, Apple Music y Tidal solo aportan los datos de la canción: el audio se busca en YouTube.</desc>',
           defs(), f'<rect width="{w}" height="372" rx="26" fill="url(#bg)"/>']
    for row, (label, note, items) in enumerate([
        ("DESCARGA DIRECTA", "Resonda baja el archivo del propio servicio", DIRECT),
        ("CATÁLOGOS DE MÚSICA", "Solo se leen los datos; el audio se busca y se baja de YouTube", CATALOGS),
    ]):
        y = 34 + row * 168
        out.append(text(x0, y + 14, label, 14, 700, "#ff4d9a"))
        out.append(text(x0 + 190 if row == 0 else x0 + 215, y + 14, note, 14, 500, "#9d97b3"))
        for i, (name, file, caption) in enumerate(items):
            out.append(card(x0 + i * (cw + gap), y + 34, cw, 90, name, file, caption))
    out.append("</svg>")
    return "\n".join(out)


def box(x, y, w, h, title, lines, accent=False):
    stroke = "url(#wave)" if accent else "#ffffff"
    so = 1 if accent else 0.18
    s = f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="16" fill="#ffffff" fill-opacity="0.05" stroke="{stroke}" stroke-opacity="{so}" stroke-width="{2 if accent else 1}"/>'
    s += text(x + w / 2, y + 36, title, 19, 700, "#ffffff", "middle")
    for i, line in enumerate(lines):
        s += text(x + w / 2, y + 62 + i * 20, line, 14, 500, "#c9c3d9", "middle")
    return s


def arrow(x1, y1, x2, y2):
    return f'<path d="M{x1} {y1}L{x2} {y2}" stroke="#ff4d9a" stroke-width="2" fill="none" stroke-linecap="round" marker-end="url(#arrow)"/>'


def how_svg():
    w, h = 1200, 392
    bw, bh, gap = 190, 106, 50
    x = [25 + i * (bw + gap) for i in range(5)]
    y1, y2 = 74, 262
    out = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-labelledby="t d">',
           '<title id="t">Cómo funciona Resonda</title>',
           '<desc id="d">Un enlace pasa por un extractor, se descarga por trozos, se convierte al formato elegido y se guarda con sus etiquetas. Los enlaces de catálogos de música se buscan antes en YouTube.</desc>',
           defs(), f'<rect width="{w}" height="{h}" rx="26" fill="url(#bg)"/>',
           text(25 + 4, 44, "ENLACE DIRECTO", 14, 700, "#ff4d9a"),
           text(25 + 4, 232, "CATÁLOGO DE MÚSICA", 14, 700, "#ff4d9a")]
    steps = [
        ("Enlace", ["YouTube, TikTok,", "Twitch o Bandcamp"]),
        ("Extractor", ["reconoce el servicio", "y lista los formatos"]),
        ("Descarga", ["por trozos, en paralelo,", "con reintentos"]),
        ("Conversión", ["MP4 · M4A · AAC", "MP3 · FLAC · WAV"]),
        ("Archivo", ["con título, artista", "y portada"]),
    ]
    for i, (title, lines) in enumerate(steps):
        out.append(box(x[i], y1, bw, bh, title, lines, accent=i == 4))
        if i:
            out.append(arrow(x[i - 1] + bw + 6, y1 + bh / 2, x[i] - 6, y1 + bh / 2))
    cat = [
        ("Enlace", ["Spotify, Deezer,", "Apple Music o Tidal"]),
        ("Metadatos", ["título, artista,", "duración y portada"]),
        ("Emparejado", ["busca en YouTube la", "grabación que encaja"]),
    ]
    for i, (title, lines) in enumerate(cat):
        out.append(box(x[i], y2, bw, bh, title, lines))
        if i:
            out.append(arrow(x[i - 1] + bw + 6, y2 + bh / 2, x[i] - 6, y2 + bh / 2))
    # del emparejado sube al extractor de YouTube
    cx = x[2] + bw + 25
    out.append(f'<path d="M{x[2] + bw + 6} {y2 + bh / 2}H{cx}V{y1 + bh + 28}H{x[1] + bw / 2}V{y1 + bh + 6}" stroke="#ff4d9a" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" marker-end="url(#arrow)"/>')
    out.append(text(cx + 14, y2 + bh / 2 - 8, "se descarga el audio", 14, 500, "#9d97b3"))
    out.append(text(cx + 14, y2 + bh / 2 + 12, "de YouTube", 14, 500, "#9d97b3"))
    out.append("</svg>")
    return "\n".join(out)


def social_svg():
    banner = (DOCS / "banner.svg").read_text(encoding="utf8")
    bars = "".join(re.findall(r'<rect x="\d+" y="[\d.]+" width="9"[^>]*/>', banner))
    # el banner mide 1200x340; la imagen social, 1280x640: la onda se reduce y se centra en los círculos
    out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 640" width="1280" height="640" role="img" aria-labelledby="t d">',
           '<title id="t">Resonda</title>',
           '<desc id="d">Descarga vídeos y música de YouTube, TikTok, Twitch, Bandcamp, Spotify, Deezer, Apple Music y Tidal. Código propio, 100 % libre.</desc>',
           defs(), '<rect width="1280" height="640" fill="url(#bg)"/>',
           '<ellipse cx="960" cy="330" rx="420" ry="260" fill="url(#glow)"/>']
    for r, o in [(120, 0.22), (170, 0.15), (225, 0.09), (285, 0.05)]:
        out.append(f'<circle cx="960" cy="330" r="{r}" fill="none" stroke="url(#wave)" stroke-width="1.5" opacity="{o}"/>')
    out.append(f'<g transform="translate(132 170) scale(0.9)">{bars}</g>')
    out.append(f'<text x="90" y="262" font-family="{FONT}" font-size="132" font-weight="800" letter-spacing="-3" fill="url(#title)">Resonda</text>')
    out.append(text(96, 330, "Vídeos y música de 8 servicios.", 36, 600, "#ece9f6"))
    out.append(text(96, 378, "Código propio · 100 % libre · MIT", 26, 500, "#b9b3cc"))
    # fila de logos
    names = DIRECT + CATALOGS
    for i, (name, file, _) in enumerate(names):
        out.append(logo(file, 96 + i * 72, 450, 40))
    out.append(text(96, 540, "YouTube · TikTok · Twitch · Bandcamp · Spotify · Deezer · Apple Music · Tidal", 20, 500, "#9d97b3"))
    out.append("</svg>")
    return "\n".join(out)


for name, content in [("services.svg", services_svg()), ("how-it-works.svg", how_svg()), ("social-preview.svg", social_svg())]:
    (DOCS / name).write_text(content + "\n", encoding="utf8")
    print("escrito docs/" + name)

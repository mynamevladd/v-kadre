"""Ecosystem design system (shared with Строчка / LiveSet), adapted for Qt.

Shared with Строчка (so the apps read as one family): near-black stage UI, one accent,
Unbounded for titles and the main button, Manrope caps-with-tracking for labels, JetBrains Mono
for numbers, header / center / right panel / footer layout.

«В кадре» own system on top of that:
- accent is peach-pink (Строчка has lime), solid — no gradients;
- "instrument" geometry: radius 6 for controls, 8 for cards;
- toggled chips/segments are outlined + tinted; the only solid accent fill is the main button;
- section labels in mono with numbers, like a camera OSD menu: 01 · ИСТОЧНИК;
- viewfinder corner brackets as the signature: logo, on-air frame, target, focused card;
- sliders have a lens-style tick scale and a needle handle.

Qt stylesheets can't do letter-spacing, so fonts are set from code via `font()` / `set_font()`,
and the stylesheet below never sets a font.
"""

from __future__ import annotations

from PySide6.QtCore import QByteArray, QRectF, Qt
from PySide6.QtGui import QColor, QFont, QFontDatabase, QIcon, QPainter, QPalette, QPixmap
from PySide6.QtWidgets import QApplication, QWidget

from speakertrack.settings import resource_dir

# ── Tokens (names follow Строчка's src/index.css) ──────────────────────────────
BG = "#000000"           # window background
SURFACE_0 = "#030303"    # right panel, bars
SURFACE_1 = "#0a0a0a"    # cards
SURFACE_2 = "#0d0d0d"    # inputs, inactive chips, secondary buttons
SURFACE_3 = "#111111"    # icon buttons
SURFACE_SEL = "#1f0f0d"  # selected row (dark peach)
LINE_SOFT = "#1a1a1a"    # panel dividers
LINE = "#1c1c1c"         # card border
LINE_STRONG = "#242424"  # input / chip border
LINE_BTN = "#2a2a2a"     # buttons on dark base
LINE_HOVER = "#3a3a3a"   # hover = border one step lighter
TEXT = "#f2f2f2"
TEXT_2 = "#d9d9d9"
TEXT_3 = "#bdbdbd"       # inactive buttons and chips
TEXT_MUTED = "#8c8c8c"   # captions, meta
TEXT_FAINT = "#6e6e6e"   # placeholders, decorative
ACCENT = "#ff9a8b"       # peach-pink — the one accent
ACCENT_HOVER = "#ffb4a8"
ACCENT_TINT = "rgba(255, 154, 139, 32)"  # toggled chip background
ON_ACCENT = "#3a0d08"    # text on accent fill
LIVE = "#ff3b3b"         # ON AIR dot only
WARN = "#ffb547"
DANGER = "#ff5a5a"
DANGER_FILL = "#e0262b"

OK = ACCENT

# ── Preview overlays ───────────────────────────────────────────────────────────
C_CROP = ACCENT          # what is on air
C_TARGET = TEXT           # the speaker
C_PERSON = TEXT_FAINT     # other people
C_ZONE = TEXT_MUTED       # stage zone (dashed)

# ── Fonts ──────────────────────────────────────────────────────────────────────
DISPLAY = "Unbounded"
UI = "Manrope"
MONO = "JetBrains Mono"


def load_fonts():
    for f in (resource_dir() / "speakertrack" / "ui" / "fonts").glob("*.ttf"):
        QFontDatabase.addApplicationFont(str(f))


def font(role: str = "body", size: int | None = None) -> QFont:
    """Roles: display, caps, eyebrow, body, meta, mono."""
    spec = {
        #           family  size weight caps  tracking(em)
        "display": (DISPLAY, 18, 900, True, 0.02),
        "button":  (DISPLAY, 12, 900, True, 0.06),
        "caps":    (UI, 11, 800, True, 0.10),
        "eyebrow": (MONO, 10, 800, True, 0.12),
        "body":    (UI, 13, 700, False, 0.0),
        "meta":    (UI, 12, 500, False, 0.0),
        "mono":    (MONO, 11, 800, False, 0.0),
    }[role]
    family, px, weight, caps, track = spec
    px = size or px
    f = QFont(family)
    f.setFamilies([family, "Segoe UI", "Segoe UI Symbol"])  # fallback for ● → ×
    f.setPixelSize(px)
    f.setWeight(QFont.Weight(weight))
    if caps:
        f.setCapitalization(QFont.AllUppercase)
    if track:
        f.setLetterSpacing(QFont.AbsoluteSpacing, round(px * track, 1))
    f.setHintingPreference(QFont.PreferNoHinting)
    return f


def set_font(w: QWidget, role: str, size: int | None = None) -> QWidget:
    w.setFont(font(role, size))
    return w


# ── Icons (Lucide, stroke = currentColor) ──────────────────────────────────────
_ICONS = {
    "play": '<polygon points="6 3 20 12 6 21 6 3"/>',
    "stop": '<rect width="14" height="14" x="5" y="5" rx="2"/>',
    "wide": '<path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/>'
            '<path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/>',
    "target": '<circle cx="12" cy="12" r="10"/><line x1="22" x2="18" y1="12" y2="12"/>'
              '<line x1="6" x2="2" y1="12" y2="12"/><line x1="12" x2="12" y1="6" y2="2"/>'
              '<line x1="12" x2="12" y1="22" y2="18"/>',
    "zone": '<path d="M5 3a2 2 0 0 0-2 2"/><path d="M19 3a2 2 0 0 1 2 2"/><path d="M21 19a2 2 0 0 1-2 2"/>'
            '<path d="M5 21a2 2 0 0 1-2-2"/><path d="M9 3h1"/><path d="M9 21h1"/><path d="M14 3h1"/>'
            '<path d="M14 21h1"/><path d="M3 9v1"/><path d="M21 9v1"/><path d="M3 14v1"/><path d="M21 14v1"/>',
    "reset": '<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>',
    "refresh": '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>'
               '<path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>',
    "plug": '<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/>'
            '<path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>',
    "user": '<circle cx="12" cy="8" r="4.5"/><path d="M20 21a8 8 0 0 0-16 0"/>',
    "chevron": '<path d="m6 9 6 6 6-6"/>',
    "check": '<path d="M20 6 9 17l-5-5"/>',
}


def _icon_pixmap(name: str, color: str, size: int) -> QPixmap:
    from PySide6.QtSvg import QSvgRenderer
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="{color}" '
           f'stroke-width="2" stroke-linecap="round" stroke-linejoin="round">{_ICONS[name]}</svg>')
    ratio = 2
    pm = QPixmap(size * ratio, size * ratio)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    QSvgRenderer(QByteArray(svg.encode())).render(p, QRectF(0, 0, size * ratio, size * ratio))
    p.end()
    pm.setDevicePixelRatio(ratio)
    return pm


def icon(name: str, color: str = TEXT_3, active: str | None = None, size: int = 16) -> QIcon:
    """Icon with an optional color for the checked/active state."""
    ic = QIcon()
    ic.addPixmap(_icon_pixmap(name, color, size), QIcon.Normal, QIcon.Off)
    ic.addPixmap(_icon_pixmap(name, active or color, size), QIcon.Normal, QIcon.On)
    ic.addPixmap(_icon_pixmap(name, TEXT_FAINT, size), QIcon.Disabled, QIcon.Off)
    return ic


def _icon_file(name: str, color: str) -> str:
    """Icon as a temp PNG file — stylesheets can only reference files."""
    import tempfile
    from pathlib import Path
    path = Path(tempfile.gettempdir()) / f"speakertrack-{name}-{color.strip('#')}.png"
    if not path.exists():
        pm = _icon_pixmap(name, color, 12)
        pm.setDevicePixelRatio(1)
        pm.save(str(path))
    return path.as_posix()


# ── Stylesheet (no fonts here — see font()) ────────────────────────────────────
def stylesheet() -> str:
    chevron = _icon_file("chevron", TEXT_MUTED)
    check = _icon_file("check", ON_ACCENT)
    return f"""
QWidget {{ background: transparent; color: {TEXT}; }}
QMainWindow, QDialog, QMessageBox {{ background: {BG}; }}
QFrame#header {{ background: {BG}; border-bottom: 1px solid {LINE_SOFT}; }}
QFrame#footer {{ background: {SURFACE_0}; border-top: 1px solid {LINE_SOFT}; }}
QFrame#panel {{ background: {SURFACE_0}; border-left: 1px solid {LINE_SOFT}; }}
QFrame#card {{ background: {SURFACE_1}; border: 1px solid {LINE}; border-radius: 8px; }}
QFrame#divider {{ background: {LINE_SOFT}; border: none; max-height: 1px; min-height: 1px; }}
QFrame#vline {{ background: {LINE_SOFT}; border: none; max-width: 1px; min-width: 1px; }}

QLabel#muted {{ color: {TEXT_MUTED}; }}
QLabel#faint {{ color: {TEXT_FAINT}; }}
QLabel#eyebrow {{ color: {ACCENT}; }}
QLabel#value {{ color: {TEXT}; }}

/* secondary button */
QPushButton {{
    background: {SURFACE_2}; color: {TEXT_3}; border: 1px solid {LINE_BTN};
    border-radius: 6px; padding: 0 12px; min-height: 32px;
}}
QPushButton:hover {{ border-color: {LINE_HOVER}; color: {TEXT}; }}
QPushButton:disabled {{ color: {TEXT_FAINT}; border-color: {LINE}; }}
/* chip / segment toggled: outline + tint (solid fill is reserved for the main button) */
QPushButton[chip="true"] {{ border-color: {LINE_STRONG}; border-radius: 6px; min-height: 30px; padding: 0 12px; }}
QPushButton[chip="true"]:checked, QPushButton[seg="true"]:checked {{
    background: {ACCENT_TINT}; border-color: {ACCENT}; color: {ACCENT};
}}
QPushButton[seg="true"] {{ border-color: {LINE_STRONG}; border-radius: 6px; min-height: 30px; padding: 0 4px; }}
/* main action: the only solid accent fill */
QPushButton#primary {{
    background: {ACCENT}; color: {ON_ACCENT}; border: none; border-radius: 6px; padding: 0 18px; min-height: 32px;
}}
QPushButton#primary:hover {{ background: {ACCENT_HOVER}; }}
QPushButton#primary[running="true"] {{ background: {DANGER_FILL}; color: white; }}
QPushButton#primary[running="true"]:hover {{ background: #f0353a; }}
/* icon-only */
QPushButton#icon {{ background: {SURFACE_3}; border: 1px solid {LINE_STRONG}; padding: 0; min-width: 32px; max-width: 32px; }}
QPushButton#icon:hover {{ border-color: {LINE_HOVER}; }}

QComboBox, QLineEdit, QSpinBox, QDoubleSpinBox {{
    background: {SURFACE_2}; color: {TEXT}; border: 1px solid {LINE_STRONG};
    border-radius: 6px; padding: 0 10px; min-height: 34px;
    selection-background-color: {ACCENT}; selection-color: {ON_ACCENT};
}}
QComboBox:hover, QLineEdit:hover, QSpinBox:hover, QDoubleSpinBox:hover {{ border-color: {LINE_HOVER}; }}
QComboBox:focus, QLineEdit:focus, QSpinBox:focus, QDoubleSpinBox:focus {{ border-color: {ACCENT}; }}
QComboBox:disabled, QLineEdit:disabled, QSpinBox:disabled, QDoubleSpinBox:disabled {{
    color: {TEXT_FAINT}; border-color: {LINE};
}}
QComboBox::drop-down {{ border: none; width: 26px; }}
QComboBox::down-arrow {{ image: url({chevron}); width: 12px; height: 12px; }}
QComboBox QAbstractItemView {{
    background: {SURFACE_1}; border: 1px solid {LINE_STRONG}; outline: none; padding: 4px;
    selection-background-color: {SURFACE_SEL}; selection-color: {ACCENT};
}}
QSpinBox::up-button, QSpinBox::down-button, QDoubleSpinBox::up-button, QDoubleSpinBox::down-button {{
    width: 0; border: none;
}}

QCheckBox {{ spacing: 10px; color: {TEXT_2}; }}
QCheckBox::indicator {{
    width: 16px; height: 16px; border-radius: 4px; border: 1px solid {LINE_BTN}; background: {SURFACE_2};
}}
QCheckBox::indicator:checked {{ background: {ACCENT}; border-color: {ACCENT}; image: url({check}); }}

/* lens-style slider: thin track, needle handle (tick scale is drawn by widgets.TickScale) */
QSlider {{ min-height: 18px; }}
QSlider::groove:horizontal {{ height: 2px; background: {LINE_BTN}; }}
QSlider::sub-page:horizontal {{ background: {ACCENT}; }}
QSlider::handle:horizontal {{
    background: {TEXT}; width: 4px; height: 18px; margin: -8px 0; border-radius: 2px;
}}
QSlider::handle:horizontal:hover {{ background: {ACCENT}; }}
QSlider::sub-page:horizontal:disabled {{ background: {TEXT_FAINT}; }}

QScrollArea {{ border: none; }}
QScrollBar:vertical {{ background: transparent; width: 6px; margin: 0; }}
QScrollBar::handle:vertical {{ background: {LINE_HOVER}; border-radius: 3px; min-height: 30px; }}
QScrollBar::handle:vertical:hover {{ background: #4a4a4a; }}
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical,
QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical {{ height: 0; background: none; }}
QToolTip {{ background: {SURFACE_1}; color: {TEXT}; border: 1px solid {LINE_STRONG}; padding: 6px; }}
QMessageBox QLabel {{ color: {TEXT_2}; }}
"""


def apply(app: QApplication):
    load_fonts()
    app.setStyle("Fusion")
    pal = QPalette()
    for role, c in ((QPalette.Window, BG), (QPalette.WindowText, TEXT), (QPalette.Base, SURFACE_2),
                    (QPalette.AlternateBase, SURFACE_1), (QPalette.Text, TEXT), (QPalette.Button, SURFACE_2),
                    (QPalette.ButtonText, TEXT_3), (QPalette.Highlight, ACCENT),
                    (QPalette.HighlightedText, ON_ACCENT), (QPalette.ToolTipBase, SURFACE_1),
                    (QPalette.ToolTipText, TEXT), (QPalette.PlaceholderText, TEXT_FAINT)):
        pal.setColor(role, QColor(c))
    app.setPalette(pal)
    app.setFont(font("body"))
    app.setFont(font("caps"), "QPushButton")
    app.setFont(font("mono", 12), "QSpinBox")
    app.setFont(font("mono", 12), "QDoubleSpinBox")
    app.setStyleSheet(stylesheet())

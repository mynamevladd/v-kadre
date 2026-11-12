"""Reusable controls: ecosystem base (Строчка's controls.js) + «В кадре» instrument look."""

from __future__ import annotations

from PySide6.QtCore import QEvent, QObject, QPointF, QRect, QRectF, QSize, Qt, Signal
from PySide6.QtGui import QColor, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import (QAbstractButton, QAbstractScrollArea, QAbstractSpinBox, QApplication,
                               QButtonGroup, QColorDialog, QComboBox, QFrame, QHBoxLayout, QLabel, QLayout,
                               QPushButton, QSizePolicy, QSlider, QStyle, QStyleOptionSlider, QVBoxLayout, QWidget)

from speakertrack.ui import theme


def draw_brackets(p: QPainter, r: QRectF, length: float, pen: QPen):
    """Viewfinder corners — the app's signature shape."""
    p.setPen(pen)
    p.setBrush(Qt.NoBrush)
    l = min(length, r.width() / 3, r.height() / 3)
    x0, y0, x1, y1 = r.left(), r.top(), r.right(), r.bottom()
    for (cx, cy, dx, dy) in ((x0, y0, 1, 1), (x1, y0, -1, 1), (x0, y1, 1, -1), (x1, y1, -1, -1)):
        path = QPainterPath(QPointF(cx + dx * l, cy))
        path.lineTo(cx, cy)
        path.lineTo(cx, cy + dy * l)
        p.drawPath(path)


class LogoMark(QWidget):
    """Accent viewfinder with a speaker inside: corner brackets + head + shoulders."""

    def __init__(self, size=26, parent=None):
        super().__init__(parent)
        self.setFixedSize(size, size)

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        s = self.width()
        pen = QPen(QColor(theme.ACCENT), s * 0.11, Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin)
        m = s * 0.08
        draw_brackets(p, QRectF(m, m, s - 2 * m, s - 2 * m), s * 0.3, pen)
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.ACCENT))
        p.drawEllipse(QPointF(s / 2, s * 0.40), s * 0.12, s * 0.12)
        body = QPainterPath()
        body.moveTo(s * 0.28, s * 0.74)
        body.quadTo(s * 0.30, s * 0.56, s / 2, s * 0.56)
        body.quadTo(s * 0.70, s * 0.56, s * 0.72, s * 0.74)
        body.closeSubpath()
        p.drawPath(body)
        p.end()


class Logo(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(10)
        lay.addWidget(LogoMark(26))
        word = QLabel("<span style='color:%s'>В</span>&nbsp;<span>КАДРЕ</span>" % theme.ACCENT)
        theme.set_font(word, "display", 15)
        lay.addWidget(word)


def caps_label(text: str, role="caps", obj="muted") -> QLabel:
    lbl = QLabel(text)
    lbl.setObjectName(obj)
    theme.set_font(lbl, role)
    return lbl


def divider() -> QFrame:
    f = QFrame()
    f.setObjectName("divider")
    return f


def vline() -> QFrame:
    f = QFrame()
    f.setObjectName("vline")
    return f


class Card(QFrame):
    """Group: caps label above a card; rows inside are separated by 1px dividers."""

    def __init__(self, title: str | None = None):
        super().__init__()
        self.setObjectName("card")
        self.lay = QVBoxLayout(self)
        self.lay.setContentsMargins(14, 6, 14, 6)
        self.lay.setSpacing(0)
        self._rows = 0
        self.title = title
        self._focused = False
        app = QApplication.instance()
        if app:
            app.focusChanged.connect(self._on_focus)

    def _on_focus(self, _old, new):
        inside = new is not None and self.isAncestorOf(new)
        if inside != self._focused:
            self._focused = inside
            self.update()

    def paintEvent(self, e):
        super().paintEvent(e)
        if self._focused:
            # signature: viewfinder corners mark the card you are editing
            p = QPainter(self)
            p.setRenderHint(QPainter.Antialiasing)
            draw_brackets(p, QRectF(self.rect()).adjusted(1, 1, -1, -1), 14,
                          QPen(QColor(theme.ACCENT), 2, Qt.SolidLine, Qt.SquareCap))
            p.end()

    def add(self, w: QWidget | QLayout, pad=(10, 10)):
        if self._rows:
            self.lay.addWidget(divider())
        box = QWidget()
        bl = QVBoxLayout(box)
        bl.setContentsMargins(0, pad[0], 0, pad[1])
        bl.setSpacing(8)
        if isinstance(w, QLayout):
            bl.addLayout(w)
        else:
            bl.addWidget(w)
        self.lay.addWidget(box)
        self._rows += 1
        return box

    def row(self, label: str, control: QWidget, hint: str | None = None):
        """Label (and hint) left, control right."""
        h = QHBoxLayout()
        h.setSpacing(12)
        left = QVBoxLayout()
        left.setSpacing(2)
        lbl = QLabel(label)
        theme.set_font(lbl, "body", 13)
        left.addWidget(lbl)
        if hint:
            hl = QLabel(hint)
            hl.setObjectName("muted")
            hl.setWordWrap(True)
            theme.set_font(hl, "meta", 11)
            left.addWidget(hl)
        h.addLayout(left, 1)
        h.addWidget(control, 0, Qt.AlignVCenter)
        return self.add(h)

    def field(self, label: str, control: QWidget, right: QWidget | None = None):
        """Caps label (with optional right part) above a full-width control."""
        v = QVBoxLayout()
        v.setSpacing(8)
        top = QHBoxLayout()
        top.addWidget(caps_label(label))
        top.addStretch(1)
        if right:
            top.addWidget(right)
        v.addLayout(top)
        v.addWidget(control)
        return self.add(v)


def section(title: str, card: Card, num: int | None = None) -> QWidget:
    """Mono OSD-style label with a number: 01 · ИСТОЧНИК."""
    w = QWidget()
    v = QVBoxLayout(w)
    v.setContentsMargins(0, 0, 0, 0)
    v.setSpacing(8)
    text = f"<span style='color:{theme.ACCENT}'>{num:02d}</span>&nbsp;·&nbsp;{title}" if num else title
    v.addWidget(caps_label(text, "eyebrow"))
    v.addWidget(card)
    return w


class Switch(QAbstractButton):
    """Track 44×24, knob 18. Off: dark track; on: accent track, dark knob."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setCheckable(True)
        self.setCursor(Qt.PointingHandCursor)
        self.setFixedSize(44, 24)

    def sizeHint(self):
        return QSize(44, 24)

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        on = self.isChecked()
        if not self.isEnabled():
            p.setOpacity(0.4)
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.ACCENT) if on else QColor(theme.LINE_BTN))
        p.drawRoundedRect(QRectF(0, 0, 44, 24), 12, 12)
        p.setBrush(QColor(theme.ON_ACCENT if on else theme.TEXT_MUTED))
        p.drawEllipse(QRectF(23 if on else 3, 3, 18, 18))
        p.end()


class WheelGuard(QObject):
    """App-wide filter: the mouse wheel scrolls the settings page instead of changing values.

    Sliders and combo boxes never take the wheel; a spin box takes it only while it has focus
    (you clicked into it). The wheel event goes to the enclosing scroll area instead.
    """

    def eventFilter(self, obj, e):
        if e.type() != QEvent.Wheel or not isinstance(obj, QWidget):
            return False
        if isinstance(obj, QAbstractSpinBox):
            if obj.hasFocus():
                return False
        elif not isinstance(obj, (QSlider, QComboBox)):
            return False  # everything else (scroll bars included) keeps the wheel
        w = obj.parentWidget()
        while w is not None and not isinstance(w, QAbstractScrollArea):
            w = w.parentWidget()
        if w is not None:
            QApplication.sendEvent(w.verticalScrollBar(), e)
        return True


def _slider_value_at(slider: QSlider, x: float) -> int:
    """Slider value under widget x, centring the handle on the cursor."""
    opt = QStyleOptionSlider()
    slider.initStyleOption(opt)
    st = slider.style()
    groove = st.subControlRect(QStyle.CC_Slider, opt, QStyle.SC_SliderGroove, slider)
    handle = st.subControlRect(QStyle.CC_Slider, opt, QStyle.SC_SliderHandle, slider)
    span = max(groove.width() - handle.width(), 1)
    pos = int(round(x - groove.x() - handle.width() / 2))
    return QStyle.sliderValueFromPosition(slider.minimum(), slider.maximum(), max(0, min(pos, span)), span)


class Slider(QSlider):
    """Click anywhere on the track: the handle jumps there and keeps following the mouse."""

    def __init__(self, parent=None):
        super().__init__(Qt.Horizontal, parent)
        self.setFocusPolicy(Qt.ClickFocus)
        self._drag = False

    def mousePressEvent(self, e):
        if e.button() != Qt.LeftButton:
            return super().mousePressEvent(e)
        self._drag = True
        self.setSliderDown(True)
        self.setValue(_slider_value_at(self, e.position().x()))
        e.accept()

    def mouseMoveEvent(self, e):
        if self._drag:
            self.setValue(_slider_value_at(self, e.position().x()))
            e.accept()
        else:
            super().mouseMoveEvent(e)

    def mouseReleaseEvent(self, e):
        if self._drag and e.button() == Qt.LeftButton:
            self._drag = False
            self.setSliderDown(False)
            e.accept()
        else:
            super().mouseReleaseEvent(e)

    def wheelEvent(self, e):
        e.ignore()


class TickScale(QWidget):
    """Lens-style scale under a slider: small ticks, every 5th taller, filled ones in accent.
    Also a hit area: click / drag on the scale moves the slider."""

    def __init__(self, slider: QSlider, ticks=21):
        super().__init__()
        self.slider, self.ticks = slider, ticks
        self.setFixedHeight(10)
        self.setCursor(Qt.PointingHandCursor)
        slider.valueChanged.connect(self.update)

    def _drag(self, e):
        if self.slider.isEnabled():
            self.slider.setValue(_slider_value_at(self.slider, e.position().x()))

    def mousePressEvent(self, e):
        if e.button() == Qt.LeftButton:
            self._drag(e)

    def mouseMoveEvent(self, e):
        if e.buttons() & Qt.LeftButton:
            self._drag(e)

    def paintEvent(self, _):
        p = QPainter(self)
        lo, hi = self.slider.minimum(), self.slider.maximum()
        frac = (self.slider.value() - lo) / max(hi - lo, 1)
        x0, x1 = 2, self.width() - 3          # matches the 4px needle handle travel
        for i in range(self.ticks):
            t = i / (self.ticks - 1)
            x = round(x0 + t * (x1 - x0))
            major = i % 5 == 0
            c = theme.ACCENT if t <= frac + 1e-6 and self.slider.isEnabled() else (
                theme.TEXT_FAINT if major else theme.LINE_HOVER)
            p.setPen(QPen(QColor(c), 1))
            p.drawLine(QPointF(x, 0), QPointF(x, 6 if major else 3))
        p.end()


class SliderRow(QWidget):
    """Caps label + mono value on top, slider below. Bound to a float settings field."""

    def __init__(self, settings, attr, label, lo, hi, step, fmt="{:.2f}", on_change=None, hint=None):
        super().__init__()
        self.s, self.attr, self.lo, self.step, self.fmt = settings, attr, lo, step, fmt
        self.on_change = on_change
        v = QVBoxLayout(self)
        v.setContentsMargins(0, 0, 0, 0)
        v.setSpacing(6)
        top = QHBoxLayout()
        top.addWidget(caps_label(label))
        top.addStretch(1)
        self.value = QLabel()
        self.value.setObjectName("value")
        theme.set_font(self.value, "mono", 12)
        top.addWidget(self.value)
        v.addLayout(top)
        self.slider = Slider()
        self.slider.setRange(0, int(round((hi - lo) / step)))
        self.slider.setCursor(Qt.PointingHandCursor)
        v.addWidget(self.slider)
        v.addWidget(TickScale(self.slider))
        if hint:
            self.setToolTip(hint)
        self.load()
        self.slider.valueChanged.connect(self._changed)

    def load(self):
        val = getattr(self.s, self.attr)
        self.slider.blockSignals(True)
        self.slider.setValue(int(round((val - self.lo) / self.step)))
        self.slider.blockSignals(False)
        self.value.setText(self.fmt.format(val))

    def _changed(self, i):
        val = round(self.lo + i * self.step, 4)
        setattr(self.s, self.attr, val)
        self.value.setText(self.fmt.format(val))
        if self.on_change:
            self.on_change()


class Segmented(QWidget):
    """Row of exclusive chips."""
    changed = Signal(int)

    def __init__(self, labels: list[str], parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(4)
        self.group = QButtonGroup(self)
        self.group.setExclusive(True)
        for i, text in enumerate(labels):
            b = QPushButton(text)
            b.setCheckable(True)
            b.setProperty("seg", True)
            b.setCursor(Qt.PointingHandCursor)
            b.setFont(theme.font("caps", 10))
            b.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Fixed)
            self.group.addButton(b, i)
            lay.addWidget(b)
        self.group.idClicked.connect(self.changed)

    def set_index(self, i: int):
        b = self.group.button(i)
        if b:
            b.setChecked(True)

    def index(self) -> int:
        return self.group.checkedId()


class _Swatch(QAbstractButton):
    """22px color dot; checked = accent ring around it. color=None draws the '+' (custom) dot."""

    def __init__(self, color: str | None, parent=None):
        super().__init__(parent)
        self.color = color
        self.setCheckable(True)
        self.setCursor(Qt.PointingHandCursor)
        self.setFixedSize(30, 30)

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        c = QPointF(15, 15)
        if self.isChecked():
            p.setPen(QPen(QColor(theme.TEXT), 2))
            p.setBrush(Qt.NoBrush)
            p.drawEllipse(c, 13, 13)
        if self.color:
            p.setPen(QPen(QColor(0, 0, 0, 60), 1))
            p.setBrush(QColor(self.color))
            p.drawEllipse(c, 10, 10)
        else:
            p.setPen(QPen(QColor(theme.LINE_HOVER if not self.underMouse() else theme.TEXT_3), 1.5, Qt.DashLine))
            p.setBrush(Qt.NoBrush)
            p.drawEllipse(c, 10, 10)
            p.setPen(QPen(QColor(theme.TEXT_3), 1.5))
            p.drawLine(QPointF(11, 15), QPointF(19, 15))
            p.drawLine(QPointF(15, 11), QPointF(15, 19))
        p.end()


class ColorSwatches(QWidget):
    """Preset color dots + a '+' dot that opens a color picker. Emits '#rrggbb'."""
    changed = Signal(str)

    def __init__(self, presets: list[str], parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(2)
        self.group = QButtonGroup(self)
        self.group.setExclusive(True)
        self._presets = [c.lower() for c in presets]
        for c in self._presets:
            b = _Swatch(c)
            b.setToolTip(c)
            self.group.addButton(b)
            lay.addWidget(b)
            b.clicked.connect(lambda _=False, col=c: self.changed.emit(col))
        self.custom = _Swatch(None)
        self.custom.setToolTip("Свой цвет…")
        self.group.addButton(self.custom)
        lay.addWidget(self.custom)
        lay.addStretch(1)
        self.custom.clicked.connect(self._pick)
        self._value = self._presets[0]

    def set_color(self, color: str):
        self._value = color.lower()
        for b in self.group.buttons():
            if b.color == self._value:
                b.setChecked(True)
                break
        else:
            self.custom.color = self._value   # custom color shows in the last dot
            self.custom.setChecked(True)
            self.custom.update()

    def _pick(self):
        col = QColorDialog.getColor(QColor(self._value), self.window(), "Цвет плашки")
        if col.isValid():
            self.set_color(col.name())
            self.changed.emit(col.name())
        else:
            self.set_color(self._value)       # restore the previous check


def chip(text: str, icon_name: str | None = None, checkable=True) -> QPushButton:
    b = QPushButton(f" {text}" if icon_name else text)
    b.setProperty("chip", True)
    b.setFont(theme.font("caps", 10))
    b.setCheckable(checkable)
    b.setCursor(Qt.PointingHandCursor)
    if icon_name:
        b.setIcon(theme.icon(icon_name, theme.TEXT_3, theme.ACCENT, 14))
        b.setIconSize(QSize(14, 14))
    return b


def icon_button(icon_name: str, tip: str) -> QPushButton:
    b = QPushButton()
    b.setObjectName("icon")
    b.setIcon(theme.icon(icon_name, theme.TEXT_3, size=16))
    b.setIconSize(QSize(16, 16))
    b.setToolTip(tip)
    b.setAccessibleName(tip)
    b.setCursor(Qt.PointingHandCursor)
    return b


class StatusPill(QWidget):
    """Height 26, radius 999, 6px dot + caps text: ● OBS 31.0"""

    def __init__(self, text="", color=theme.TEXT_FAINT, parent=None):
        super().__init__(parent)
        self.setFixedHeight(26)
        self._text, self._color, self._glow = text, color, False
        self.setFont(theme.font("caps", 10))

    def set(self, text: str, color: str, glow=False):
        self._text, self._color, self._glow = text, color, glow
        self.updateGeometry()
        self.update()

    def sizeHint(self):
        return QSize(self.fontMetrics().horizontalAdvance(self._text.upper()) + 36, 26)

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        r = QRectF(self.rect()).adjusted(0.5, 0.5, -0.5, -0.5)
        p.setPen(QPen(QColor(theme.LINE_STRONG), 1))
        p.setBrush(Qt.NoBrush)
        p.drawRoundedRect(r, 13, 13)
        c = QColor(self._color)
        if self._glow:
            for rad, a in ((7, 40), (5, 70)):
                g = QColor(c)
                g.setAlpha(a)
                p.setPen(Qt.NoPen)
                p.setBrush(g)
                p.drawEllipse(QPointF(15, 13), rad, rad)
        p.setPen(Qt.NoPen)
        p.setBrush(c)
        p.drawEllipse(QPointF(15, 13), 3, 3)
        p.setPen(QColor(theme.TEXT_2))
        p.drawText(QRectF(25, 0, self.width() - 25, 26), Qt.AlignVCenter | Qt.AlignLeft, self._text)
        p.end()


class FlowLayout(QLayout):
    """Wrapping row of widgets (scene chips)."""

    def __init__(self, parent=None, spacing=6):
        super().__init__(parent)
        self._items = []
        self._sp = spacing
        self.setContentsMargins(0, 0, 0, 0)

    def addItem(self, item):
        self._items.append(item)

    def count(self):
        return len(self._items)

    def itemAt(self, i):
        return self._items[i] if 0 <= i < len(self._items) else None

    def takeAt(self, i):
        return self._items.pop(i) if 0 <= i < len(self._items) else None

    def expandingDirections(self):
        return Qt.Orientations(0)

    def hasHeightForWidth(self):
        return True

    def heightForWidth(self, w):
        return self._do(QRect(0, 0, w, 0), True)

    def setGeometry(self, rect):
        super().setGeometry(rect)
        self._do(rect, False)

    def sizeHint(self):
        return self.minimumSize()

    def minimumSize(self):
        s = QSize()
        for it in self._items:
            s = s.expandedTo(it.minimumSize())
        return s

    def _do(self, rect, test):
        x, y, line_h = rect.x(), rect.y(), 0
        for it in self._items:
            hint = it.sizeHint()
            nx = x + hint.width() + self._sp
            if nx - self._sp > rect.right() + 1 and line_h > 0:
                x, y = rect.x(), y + line_h + self._sp
                nx = x + hint.width() + self._sp
                line_h = 0
            if not test:
                it.setGeometry(QRect(x, y, hint.width(), hint.height()))
            x, line_h = nx, max(line_h, hint.height())
        return y + line_h - rect.y()

    def clear(self):
        while self._items:
            it = self._items.pop()
            w = it.widget()
            if w:
                w.hide()
                w.setParent(None)
                w.deleteLater()

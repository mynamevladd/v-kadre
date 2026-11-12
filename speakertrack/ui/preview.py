"""Camera preview with overlays; click to lock onto a person, drag to draw the stage zone."""

from __future__ import annotations

import numpy as np
from PySide6.QtCore import QPointF, QRectF, Qt, Signal
from PySide6.QtGui import QColor, QImage, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import QSizePolicy, QWidget

from speakertrack.framing import HEAD_KP, HIP_KP, SHOULDER_KP
from speakertrack.ui import theme
from speakertrack.ui.widgets import draw_brackets

RADIUS = 8


def to_qimage(img_rgb: np.ndarray) -> QImage:
    h, w = img_rgb.shape[:2]
    rgb = np.ascontiguousarray(img_rgb)
    return QImage(rgb.data, w, h, 3 * w, QImage.Format_RGB888).copy()


def _alpha(c: str, a: int) -> QColor:
    q = QColor(c)
    q.setAlpha(a)
    return q


def draw_pill(p: QPainter, x: float, y: float, text: str, dot: str, glow=False, right=False) -> QRectF:
    """Status pill on top of the video: dark glass, 6px dot, caps text."""
    p.setFont(theme.font("caps", 10))
    w = p.fontMetrics().horizontalAdvance(text.upper()) + 34
    r = QRectF(x - w if right else x, y, w, 26)
    p.setPen(QPen(_alpha(theme.LINE_HOVER, 200), 1))
    p.setBrush(QColor(0, 0, 0, 190))
    p.drawRoundedRect(r, 13, 13)
    c = QColor(dot)
    p.setPen(Qt.NoPen)
    if glow:
        p.setBrush(_alpha(dot, 60))
        p.drawEllipse(QPointF(r.x() + 14, r.center().y()), 6, 6)
    p.setBrush(c)
    p.drawEllipse(QPointF(r.x() + 14, r.center().y()), 3, 3)
    p.setPen(QColor(theme.TEXT))
    p.drawText(r.adjusted(24, 0, 0, 0), Qt.AlignVCenter | Qt.AlignLeft, text)
    return r


class PreviewWidget(QWidget):
    clicked = Signal(float, float)               # normalized source coords
    zone_drawn = Signal(float, float, float, float)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumSize(480, 270)
        self.setSizePolicy(QSizePolicy.Expanding, QSizePolicy.Expanding)
        self.setMouseTracking(True)
        self.data = None
        self.qimg: QImage | None = None
        self.message = "Камера не запущена\nНажмите «Старт» (F5), чтобы начать слежение"
        self.zone_mode = False
        self.fault = ""
        self._drag_from: QPointF | None = None
        self._drag_to: QPointF | None = None

    # -- data ------------------------------------------------------------------------

    def set_data(self, data):
        self.data = data
        self.qimg = to_qimage(data.img)
        self.update()

    def clear(self):
        self.data = None
        self.qimg = None
        self.update()

    def set_message(self, text: str | None):
        """'Title\\nhint' or None."""
        self.message = text
        self.update()

    def set_fault(self, text: str):
        """Camera watchdog message ("" = fine): a red band over the preview."""
        self.fault = text
        self.update()

    def set_zone_mode(self, on: bool):
        self.zone_mode = on
        self.setCursor(Qt.CrossCursor if on else Qt.ArrowCursor)
        self.update()

    # -- geometry --------------------------------------------------------------------

    def image_rect(self) -> QRectF:
        if self.qimg is None:
            return QRectF(self.rect())
        iw, ih = self.qimg.width(), self.qimg.height()
        scale = min(self.width() / iw, self.height() / ih)
        w, h = iw * scale, ih * scale
        return QRectF((self.width() - w) / 2, (self.height() - h) / 2, w, h)

    def to_norm(self, pt: QPointF) -> tuple[float, float]:
        r = self.image_rect()
        x = (pt.x() - r.x()) / r.width()
        y = (pt.y() - r.y()) / r.height()
        return min(max(x, 0.0), 1.0), min(max(y, 0.0), 1.0)

    def _rect(self, r: QRectF, x0, y0, x1, y1) -> QRectF:
        return QRectF(r.x() + x0 * r.width(), r.y() + y0 * r.height(),
                      (x1 - x0) * r.width(), (y1 - y0) * r.height())

    # -- painting --------------------------------------------------------------------

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        frame = QRectF(self.rect()).adjusted(0.5, 0.5, -0.5, -0.5)
        clip = QPainterPath()
        clip.addRoundedRect(frame, RADIUS, RADIUS)
        p.setClipPath(clip)
        p.fillRect(self.rect(), QColor(theme.BG))

        d = self.data
        if self.qimg is not None and d is not None:
            r = self.image_rect()
            p.setRenderHint(QPainter.SmoothPixmapTransform)
            p.drawImage(r, self.qimg)
            crop = self._rect(r, *d.crop)

            # dim everything outside the on-air frame
            outside = QPainterPath()
            outside.addRect(r)
            hole = QPainterPath()
            hole.addRect(crop)
            p.fillPath(outside.subtracted(hole), QColor(0, 0, 0, 110 if d.active else 60))

            # stage zone (only when it is not the whole frame)
            if tuple(d.zone) != (0.0, 0.0, 1.0, 1.0):
                p.setPen(QPen(QColor(theme.C_ZONE), 1, Qt.DashLine))
                p.setBrush(Qt.NoBrush)
                p.drawRect(self._rect(r, *d.zone))

            # people
            chosen = (d.target_id, d.partner_id)
            for person in d.people:
                if person.tid in chosen:
                    continue
                box = self._rect(r, *person.box)
                draw_brackets(p, box, 12, QPen(QColor(theme.C_PERSON), 1.5, Qt.SolidLine, Qt.RoundCap))
            for person in d.people:
                if person.tid != d.partner_id or d.partner_id is None:
                    continue
                box = self._rect(r, *person.box)
                draw_brackets(p, box, 16, QPen(QColor(theme.C_TARGET), 2, Qt.SolidLine, Qt.RoundCap))
                self._tag(p, box, "Второй")
            for person in d.people:
                if person.tid != d.target_id:
                    continue
                box = self._rect(r, *person.box)
                draw_brackets(p, box, 18, QPen(QColor(theme.C_TARGET), 2.5, Qt.SolidLine, Qt.RoundCap))
                if person.kxy is not None:
                    p.setPen(Qt.NoPen)
                    p.setBrush(QColor(theme.ACCENT))
                    for i in HEAD_KP + SHOULDER_KP + HIP_KP:
                        if person.kconf[i] >= 0.5:
                            x, y = person.kxy[i]
                            p.drawEllipse(QPointF(r.x() + x * r.width(), r.y() + y * r.height()), 2.5, 2.5)
                self._tag(p, box, f"Спикер · {d.match:.0%}" if d.match > 0 else "Спикер")

            # on-air frame: thin line, thirds grid, heavy viewfinder corners
            col = theme.WARN if d.paused else theme.C_CROP
            p.setBrush(Qt.NoBrush)
            p.setPen(QPen(_alpha(col, 70), 1))
            for k in (1, 2):
                x = crop.left() + crop.width() * k / 3
                y = crop.top() + crop.height() * k / 3
                p.drawLine(QPointF(x, crop.top()), QPointF(x, crop.bottom()))
                p.drawLine(QPointF(crop.left(), y), QPointF(crop.right(), y))
            p.setPen(QPen(_alpha(col, 170), 1))
            p.drawRect(crop)
            draw_brackets(p, crop, 22, QPen(QColor(col), 3, Qt.SolidLine, Qt.RoundCap))

            # status pill: what the camera is doing + which scene drives it
            cut = {"close": " · крупный", "wide": " · средний"}.get(d.cut, "")
            if not d.active:
                pill = draw_pill(p, 14, 14, "Ожидание сцены", theme.TEXT_FAINT)
            elif d.paused:
                pill = draw_pill(p, 14, 14, "Общий план", theme.WARN)
            elif d.mode == "wide":
                pill = draw_pill(p, 14, 14, "Общий план сцены", theme.TEXT_3)
            elif d.target_id is None:
                pill = draw_pill(p, 14, 14, "Поиск спикера", theme.WARN)
            elif d.mode == "duo":
                pill = draw_pill(p, 14, 14, "Дуэт" if d.partner_id is not None else "Дуэт · ищу второго",
                                 theme.LIVE, glow=True)
            else:
                pill = draw_pill(p, 14, 14, "Слежение" + cut, theme.LIVE, glow=True)
            if d.active and d.scene:
                p.setFont(theme.font("caps", 10))
                p.setPen(QColor(theme.TEXT_2))
                p.drawText(QRectF(pill.right() + 12, pill.top(), 400, pill.height()),
                           Qt.AlignVCenter | Qt.AlignLeft, d.scene)

            # HUD
            self._hud(p, f"{d.zoom:.2f}×   {d.infer_ms:.0f} MS   {d.fps:.0f} FPS")

        elif self.message:
            self._empty(p, self.message)

        if self.message and d is not None and not d.active:
            self._center_note(p, self.message.split("\n")[0])

        if self.fault:
            self._fault_band(p)

        # zone being drawn
        if self._drag_from and self._drag_to:
            p.setPen(QPen(QColor(theme.ACCENT), 1.5, Qt.DashLine))
            p.setBrush(_alpha(theme.ACCENT, 30))
            p.drawRect(QRectF(self._drag_from, self._drag_to).normalized())
        elif self.zone_mode:
            draw_pill(p, self.width() - 14, 14, "Обведите зону сцены мышью", theme.ACCENT, right=True)

        p.setClipping(False)
        p.setPen(QPen(QColor(theme.LINE), 1))
        p.setBrush(Qt.NoBrush)
        p.drawRoundedRect(frame, RADIUS, RADIUS)
        p.end()

    def _fault_band(self, p: QPainter):
        r = QRectF(0, 0, self.width(), 44)
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.DANGER_FILL))
        p.drawRect(r)
        p.setBrush(QColor("white"))
        p.drawEllipse(QPointF(22, r.center().y()), 4, 4)
        p.setFont(theme.font("caps", 12))
        p.setPen(QColor("white"))
        p.drawText(r.adjusted(36, 0, -16, 0), Qt.AlignVCenter | Qt.AlignLeft,
                   f"{self.fault} — проверьте камеру")

    def _tag(self, p: QPainter, box: QRectF, text: str):
        p.setFont(theme.font("caps", 9))
        w = p.fontMetrics().horizontalAdvance(text.upper()) + 12
        y = box.top() - 22 if box.top() > 30 else box.top() + 6
        r = QRectF(box.left(), y, w, 18)
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.TEXT))
        p.drawRoundedRect(r, 4, 4)
        p.setPen(QColor(theme.BG))
        p.drawText(r, Qt.AlignCenter, text)

    def _hud(self, p: QPainter, text: str):
        p.setFont(theme.font("mono", 11))
        w = p.fontMetrics().horizontalAdvance(text) + 20
        r = QRectF(self.width() - w - 14, self.height() - 26 - 14, w, 26)
        p.setPen(QPen(_alpha(theme.LINE_HOVER, 200), 1))
        p.setBrush(QColor(0, 0, 0, 190))
        p.drawRoundedRect(r, 6, 6)
        p.setPen(QColor(theme.TEXT_2))
        p.drawText(r, Qt.AlignCenter, text)

    def _center_note(self, p: QPainter, text: str):
        p.setFont(theme.font("caps", 11))
        w = p.fontMetrics().horizontalAdvance(text.upper()) + 32
        r = QRectF((self.width() - w) / 2, (self.height() - 34) / 2, w, 34)
        p.setPen(QPen(QColor(theme.LINE_STRONG), 1))
        p.setBrush(QColor(0, 0, 0, 210))
        p.drawRoundedRect(r, 8, 8)
        p.setPen(QColor(theme.TEXT_2))
        p.drawText(r, Qt.AlignCenter, text)

    def _empty(self, p: QPainter, message: str):
        """Empty state: faint viewfinder, caps title, muted hint."""
        title, _, hint = message.partition("\n")
        c = QPointF(self.width() / 2, self.height() / 2 - 30)
        vf = QRectF(c.x() - 34, c.y() - 26, 68, 52)
        draw_brackets(p, vf, 14, QPen(QColor(theme.TEXT_FAINT), 2, Qt.SolidLine, Qt.RoundCap))
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.TEXT_FAINT))
        p.drawEllipse(c, 4, 4)
        p.setFont(theme.font("display", 13))
        p.setPen(QColor(theme.TEXT))
        p.drawText(QRectF(0, vf.bottom() + 22, self.width(), 22), Qt.AlignCenter, title)
        if hint:
            p.setFont(theme.font("meta", 12))
            p.setPen(QColor(theme.TEXT_MUTED))
            p.drawText(QRectF(40, vf.bottom() + 48, self.width() - 80, 40),
                       Qt.AlignHCenter | Qt.AlignTop | Qt.TextWordWrap, hint)

    # -- mouse -----------------------------------------------------------------------

    def mousePressEvent(self, e):
        if e.button() != Qt.LeftButton or self.qimg is None:
            return
        if self.zone_mode:
            self._drag_from = self._drag_to = e.position()
        else:
            self.clicked.emit(*self.to_norm(e.position()))

    def mouseMoveEvent(self, e):
        if self._drag_from is not None:
            self._drag_to = e.position()
            self.update()

    def mouseReleaseEvent(self, e):
        if self._drag_from is None:
            return
        x0, y0 = self.to_norm(self._drag_from)
        x1, y1 = self.to_norm(e.position())
        self._drag_from = self._drag_to = None
        if abs(x1 - x0) > 0.05 and abs(y1 - y0) > 0.05:
            self.zone_drawn.emit(min(x0, x1), min(y0, y1), max(x0, x1), max(y0, y1))
        self.update()


class AirWidget(QWidget):
    """Small 'on air' preview: the crop cut from the low-res detection frame.
    While tracking is live: accent viewfinder corners over a thin frame."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setFixedHeight(180)
        self.qimg = None
        self.crop = None
        self.live = False

    def set_data(self, data, qimg: QImage | None):
        self.qimg, self.crop = qimg, data.crop
        self.live = data.active and not data.paused
        self.update()

    def clear(self):
        self.qimg = self.crop = None
        self.live = False
        self.update()

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        frame = QRectF(self.rect()).adjusted(1, 1, -1, -1)
        clip = QPainterPath()
        clip.addRoundedRect(frame, 8, 8)
        p.setClipPath(clip)
        p.fillRect(self.rect(), QColor(theme.BG))
        if self.qimg is not None and self.crop is not None:
            x0, y0, x1, y1 = self.crop
            iw, ih = self.qimg.width(), self.qimg.height()
            src = QRectF(x0 * iw, y0 * ih, (x1 - x0) * iw, (y1 - y0) * ih)
            p.setRenderHint(QPainter.SmoothPixmapTransform)
            p.drawImage(QRectF(self.rect()), self.qimg, src)
        else:
            p.setFont(theme.font("caps", 10))
            p.setPen(QColor(theme.TEXT_FAINT))
            p.drawText(self.rect(), Qt.AlignCenter, "Нет сигнала")
        p.setClipping(False)
        p.setPen(QPen(QColor(theme.LINE_STRONG if self.live else theme.LINE), 1))
        p.setBrush(Qt.NoBrush)
        p.drawRoundedRect(frame, 8, 8)
        if self.live:
            draw_brackets(p, frame.adjusted(1, 1, -1, -1), 20, QPen(QColor(theme.ACCENT), 3, Qt.SolidLine, Qt.SquareCap))
        p.end()

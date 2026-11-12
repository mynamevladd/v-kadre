"""Main window, laid out like Строчка: header bar / preview in the center / settings panel right / footer."""

from __future__ import annotations

import logging

from PySide6.QtCore import QSize, Qt, QTimer, QUrl, Signal
from PySide6.QtGui import QDesktopServices, QKeySequence, QShortcut
from PySide6.QtWidgets import (QComboBox, QDoubleSpinBox, QFrame, QHBoxLayout, QLabel, QLineEdit, QMainWindow,
                               QMessageBox, QPushButton, QScrollArea, QSpinBox, QVBoxLayout, QWidget)

from speakertrack import __app_name__, __version__
from speakertrack.engine import Engine
from speakertrack.lower_third import LowerThird, anim_seconds, write_page
from speakertrack.obs_client import ObsConnection
from speakertrack.settings import Settings
from speakertrack.ui import theme
from speakertrack.ui.connection_dialog import ConnectionDialog
from speakertrack.ui.preview import AirWidget, PreviewWidget
from speakertrack.ui.widgets import (Card, ColorSwatches, Logo, Segmented, SliderRow, StatusPill, Switch, caps_label,
                                     chip, icon_button, section, vline)

log = logging.getLogger(__name__)

SHOT_CHOICES = [
    ("Грудь", "auto", "chest"),
    ("Пояс", "auto", "waist"),
    ("Бёдра", "auto", "hips"),
    ("Колено", "auto", "knee"),
    ("Фикс", "fixed", None),
]
WIDE_SHOTS = [("Бёдра", "hips"), ("Колено", "knee"), ("Рост", "full")]
SCENE_MODES = [("Выкл", None), ("Вести", "track"), ("Дуэт", "duo"), ("Общий", "wide")]
# name plate look
PLATE_COLORS = ["#ff9a8b", "#c6f36b", "#ffd84d", "#7cc8ff", "#c9a7ff", "#f4f1ef"]
PLATE_STYLES = [("Тёмный", "dark"), ("Светлый", "light"), ("Цветной", "color")]
PLATE_ANIMS = [("Шторка", "wipe"), ("Сбоку", "slide"), ("Снизу", "rise"), ("Плавно", "fade")]

IDLE_MESSAGE = "Камера не запущена\nНажмите «Старт» (F5), чтобы начать слежение"


class SceneModeRow(QWidget):
    """Scene name + mode segments (Выкл / Вести / Дуэт / Общий)."""

    def __init__(self, scene: str, mode: str | None, on_change):
        super().__init__()
        self.scene = scene
        h = QHBoxLayout(self)
        h.setContentsMargins(0, 0, 0, 0)
        h.setSpacing(10)
        name = QLabel(scene)
        name.setToolTip(scene)
        theme.set_font(name, "body", 12)
        name.setMinimumWidth(60)
        h.addWidget(name, 1)
        self.seg = Segmented([label for label, _ in SCENE_MODES])
        for b in self.seg.group.buttons():
            b.setFont(theme.font("caps", 9))
            b.setMinimumWidth(48)
            b.setFocusPolicy(Qt.NoFocus)
        self.seg.set_index(next((i for i, (_, m) in enumerate(SCENE_MODES) if m == mode), 0))
        self.seg.changed.connect(lambda i: on_change(scene, SCENE_MODES[i][1]))
        h.addWidget(self.seg)


class MainWindow(QMainWindow):
    plate_failed = Signal(str)   # from the name-plate thread

    def __init__(self, settings: Settings):
        super().__init__()
        self.s = settings
        self.engine = Engine(settings)
        self.plate = LowerThird(settings)
        self.setWindowTitle(__app_name__)
        self.resize(1440, 880)
        self.setMinimumSize(1100, 680)

        self._save_timer = QTimer(self, singleShot=True, interval=600)
        self._save_timer.timeout.connect(self._save)
        self._status_timer = QTimer(self, singleShot=True)
        self._status_timer.timeout.connect(lambda: self._status(self._idle_status))
        self._beep_timer = QTimer(self, interval=4000)
        self._beep_timer.timeout.connect(self._beep)
        self._idle_status = "Готово"
        self._scene_sources: dict[str, list[str]] = {}
        self._scenes: list[str] = []
        self._fault = ""

        self._build()
        self._wire()
        self._load_into_ui()

        # keyboard focus stays on the preview: Space must not press a focused button,
        # and the settings panel must not scroll to a focused field on start
        for b in self.findChildren(QPushButton):
            b.setFocusPolicy(Qt.NoFocus)
        self.preview.setFocusPolicy(Qt.StrongFocus)
        self.preview.setFocus()
        QTimer.singleShot(0, lambda: self._scroll.verticalScrollBar().setValue(0))

        if self.s.auto_connect:
            QTimer.singleShot(300, self.refresh_obs)

    # ------------------------------------------------------------------ layout

    def _build(self):
        central = QWidget()
        self.setCentralWidget(central)
        root = QVBoxLayout(central)
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)
        root.addWidget(self._build_header())
        body = QHBoxLayout()
        body.setSpacing(0)
        body.addWidget(self._build_center(), 1)
        body.addWidget(self._build_panel())
        root.addLayout(body, 1)
        root.addWidget(self._build_footer())

    def _build_header(self) -> QFrame:
        bar = QFrame()
        bar.setObjectName("header")
        bar.setFixedHeight(56)
        h = QHBoxLayout(bar)
        h.setContentsMargins(20, 0, 14, 0)
        h.setSpacing(8)
        h.addWidget(Logo())
        h.addSpacing(14)
        h.addWidget(vline())
        h.addSpacing(14)

        self.start_btn = QPushButton(" Старт")
        self.start_btn.setObjectName("primary")
        self.start_btn.setFont(theme.font("button", 12))
        self.start_btn.setIcon(theme.icon("play", theme.ON_ACCENT, size=14))
        self.start_btn.setIconSize(QSize(14, 14))
        self.start_btn.setMinimumWidth(124)
        self.start_btn.setFixedHeight(36)
        self.start_btn.setCursor(Qt.PointingHandCursor)
        self.start_btn.setToolTip("Запустить / остановить слежение (F5)")
        h.addWidget(self.start_btn)
        h.addSpacing(6)

        self.wide_btn = chip("Общий план", "wide")
        self.wide_btn.setToolTip("Временно уйти на общий план (пробел)")
        self.retarget_btn = chip("Сменить цель", "target", checkable=False)
        self.retarget_btn.setToolTip("Заново выбрать спикера. Можно просто кликнуть по человеку в превью.")
        self.zone_btn = chip("Зона сцены", "zone")
        self.zone_btn.setToolTip("Обведите мышью область сцены. Люди вне её игнорируются.")
        self.zone_reset_btn = icon_button("reset", "Сбросить зону")
        self.plate_btn = chip("Имя", "user", checkable=False)
        self.plate_btn.setToolTip("Показать плашку с именем спикера в OBS (N)")
        for w in (self.wide_btn, self.retarget_btn, self.zone_btn, self.zone_reset_btn, self.plate_btn):
            h.addWidget(w)
        h.addStretch(1)

        self.obs_pill = StatusPill("OBS · нет связи", theme.TEXT_FAINT)
        h.addWidget(self.obs_pill)
        self.conn_btn = icon_button("plug", "Подключение к OBS")
        h.addWidget(self.conn_btn)
        return bar

    def _build_center(self) -> QWidget:
        w = QWidget()
        v = QVBoxLayout(w)
        v.setContentsMargins(24, 18, 24, 20)
        v.setSpacing(10)

        top = QHBoxLayout()
        top.addWidget(caps_label("Камера", "eyebrow", "eyebrow"))
        top.addStretch(1)
        self.source_meta = QLabel("")
        self.source_meta.setObjectName("muted")
        theme.set_font(self.source_meta, "caps", 10)
        top.addWidget(self.source_meta)
        v.addLayout(top)

        self.preview = PreviewWidget()
        v.addWidget(self.preview, 1)
        v.addSpacing(6)

        bottom = QHBoxLayout()
        bottom.setSpacing(20)
        air_box = QVBoxLayout()
        air_box.setSpacing(8)
        air_top = QHBoxLayout()
        self.air_label = QLabel()
        theme.set_font(self.air_label, "caps", 10)
        air_top.addWidget(self.air_label)
        air_top.addStretch(1)
        self.air_meta = QLabel("")
        self.air_meta.setObjectName("muted")
        theme.set_font(self.air_meta, "mono", 11)
        air_top.addWidget(self.air_meta)
        air_box.addLayout(air_top)
        self.air = AirWidget()
        self.air.setFixedWidth(320)
        air_box.addWidget(self.air)
        bottom.addLayout(air_box)
        self._set_air_label(False)

        legend = Card()
        legend.lay.setContentsMargins(16, 8, 16, 8)
        legend.add(self._legend_items(), pad=(8, 8))
        legend.add(self._hotkeys(), pad=(10, 8))
        lw = QVBoxLayout()
        lw.setSpacing(8)
        lw.addWidget(caps_label("Обозначения", "eyebrow", "muted"))
        lw.addWidget(legend)
        lw.addStretch(1)
        bottom.addLayout(lw, 1)
        v.addLayout(bottom)
        return w

    def _legend_items(self) -> QWidget:
        w = QWidget()
        g = QVBoxLayout(w)
        g.setContentsMargins(0, 0, 0, 0)
        g.setSpacing(8)
        for color, name, desc in (
                (theme.C_CROP, "Кадр в эфире", "что увидят зрители"),
                (theme.C_TARGET, "Спикер", "за кем следит камера; в дуэте — оба"),
                (theme.C_PERSON, "Другие люди", "игнорируются"),
                (theme.C_ZONE, "Зона сцены", "вне неё никого не ищем")):
            row = QLabel(f"<span style='color:{color}'>▌</span>&nbsp; <b>{name}</b>"
                         f"<span style='color:{theme.TEXT_MUTED}'> — {desc}</span>")
            theme.set_font(row, "body", 12)
            g.addWidget(row)
        return w

    def _hotkeys(self) -> QWidget:
        w = QWidget()
        h = QHBoxLayout(w)
        h.setContentsMargins(0, 0, 0, 0)
        h.setSpacing(6)
        for key, text in (("F5", "старт / стоп"), ("ПРОБЕЛ", "общий план"), ("N", "имя"),
                          ("КЛИК", "выбрать спикера")):
            k = QLabel(key)
            theme.set_font(k, "mono", 10)
            k.setStyleSheet(f"color:{theme.TEXT}; background:{theme.SURFACE_3}; border:1px solid {theme.LINE_STRONG};"
                            "border-radius:5px; padding:2px 6px;")
            t = QLabel(text)
            t.setObjectName("muted")
            theme.set_font(t, "meta", 11)
            h.addWidget(k)
            h.addWidget(t)
            h.addSpacing(8)
        h.addStretch(1)
        return w

    def _spin(self, box, lo, hi, suffix, step=None) -> QWidget:
        box.setRange(lo, hi)
        if step:
            box.setSingleStep(step)
        box.setSuffix(suffix)
        box.setFixedWidth(92)
        box.setAlignment(Qt.AlignRight)
        return box

    def _hint(self, text: str) -> QLabel:
        hint = QLabel(text)
        hint.setObjectName("muted")
        hint.setWordWrap(True)
        theme.set_font(hint, "meta", 11)
        return hint

    def _build_panel(self) -> QFrame:
        panel = QFrame()
        panel.setObjectName("panel")
        panel.setFixedWidth(384)
        outer = QVBoxLayout(panel)
        outer.setContentsMargins(0, 18, 0, 0)
        outer.setSpacing(0)

        head = QVBoxLayout()
        head.setContentsMargins(18, 0, 18, 14)
        head.setSpacing(2)
        head.addWidget(caps_label("Настройки", "eyebrow", "eyebrow"))
        title = QLabel("Слежение")
        theme.set_font(title, "display", 18)
        head.addWidget(title)
        outer.addLayout(head)

        content = QWidget()
        pl = QVBoxLayout(content)
        pl.setContentsMargins(16, 4, 12, 20)
        pl.setSpacing(22)

        # 01 source + per-scene modes
        c = Card()
        self.source_cb = QComboBox()
        self.refresh_btn = icon_button("refresh", "Обновить список из OBS")
        c.field("Камера (источник)", self.source_cb, self.refresh_btn)
        self.modes_box = QWidget()
        self.modes_lay = QVBoxLayout(self.modes_box)
        self.modes_lay.setContentsMargins(0, 0, 0, 0)
        self.modes_lay.setSpacing(6)
        sv = QVBoxLayout()
        sv.setSpacing(8)
        sv.addWidget(caps_label("Режим в сценах"))
        sv.addWidget(self.modes_box)
        sv.addWidget(self._hint("Вести — за спикером, Дуэт — двое в кадре, Общий — вся сцена. "
                                "Строчка переключает сцены OBS — «В кадре» меняет режим вместе с ней."))
        c.add(sv)
        pl.addWidget(section("Источник", c, 1))

        # 02 shot + auto-cut
        c = Card()
        self.shot_seg = Segmented([label for label, _, _ in SHOT_CHOICES])
        c.field("План", self.shot_seg)
        self.fixed_zoom = self._spin(QDoubleSpinBox(), 1.0, 4.0, " ×", 0.1)
        self.fixed_zoom_row = c.row("Фиксированный зум", self.fixed_zoom)
        self.max_zoom = SliderRow(self.s, "max_zoom", "Макс. зум", 1.0, 4.0, 0.1, "{:.1f}×", self._changed,
                                  "2× из 4K = чистый 1080p. Больше — картинка мягче.")
        c.add(self.max_zoom)
        self.headroom = SliderRow(self.s, "headroom", "Над головой", 0.0, 0.25, 0.01, "{:.0%}", self._changed)
        c.add(self.headroom)
        pl.addWidget(section("Кадр", c, 2))

        c = Card()
        self.auto_cut = Switch()
        c.row("Два плана из одной камеры", self.auto_cut,
              "Стоит — крупно, пошёл — склейка на средний. Без наездов зума.")
        self.cut_seg = Segmented([label for label, _ in WIDE_SHOTS])
        self.cut_seg_row = c.field("Второй план", self.cut_seg)
        self.cut_hold = SliderRow(self.s, "cut_min_hold", "Не чаще, чем раз в", 2.0, 20.0, 1.0, "{:.0f} с",
                                  self._changed)
        self.cut_hold_row = c.add(self.cut_hold)
        pl.addWidget(section("Монтаж", c, 3))

        # 04 motion
        c = Card()
        self.smooth = SliderRow(self.s, "smooth_time", "Плавность", 0.2, 2.5, 0.05, "{:.2f} с", self._changed,
                                "Больше — мягче, но с отставанием")
        self.deadzone = SliderRow(self.s, "deadzone_x", "Мёртвая зона", 0.0, 0.35, 0.01, "{:.0%}", self._changed,
                                  "Насколько спикер может сместиться, прежде чем кадр поедет")
        self.speed = SliderRow(self.s, "max_speed", "Скорость панорамы", 0.05, 1.0, 0.05, "{:.2f}", self._changed,
                               "Максимальная скорость (доля кадра в секунду)")
        self.zoom_smooth = SliderRow(self.s, "zoom_smooth_time", "Плавность зума", 0.3, 5.0, 0.1, "{:.1f} с",
                                     self._changed)
        for w in (self.smooth, self.deadzone, self.speed, self.zoom_smooth):
            c.add(w)
        pl.addWidget(section("Движение", c, 4))

        # 05 speaker
        c = Card()
        self.reid = Switch()
        c.row("Узнавать по одежде", self.reid,
              "Помнит цвета одежды спикера и возвращается к нему, если отвлёкся на другого")
        self.wide_on_lost = Switch()
        c.row("Пропал — общий план", self.wide_on_lost, "Если спикера не видно")
        self.lost_timeout = self._spin(QDoubleSpinBox(), 0.5, 30, " с")
        c.row("Через", self.lost_timeout)
        pl.addWidget(section("Спикер", c, 5))

        # 06 name plate
        c = Card()
        self.lt_name = QLineEdit()
        self.lt_name.setPlaceholderText("Алексей Смирнов")
        c.field("Имя на плашке", self.lt_name)
        self.lt_title = QLineEdit()
        self.lt_title.setPlaceholderText("Пастор церкви")
        c.field("Подпись", self.lt_title)
        self.lt_auto = Switch()
        c.row("Показывать сама", self.lt_auto, "Когда спикер появился в кадре в начале сцены")
        self.lt_duration = SliderRow(self.s, "lt_duration", "На экране", 3.0, 15.0, 1.0, "{:.0f} с", self._changed)
        c.add(self.lt_duration)
        self.lt_color = ColorSwatches(PLATE_COLORS)
        c.field("Цвет", self.lt_color)
        self.lt_plate = Segmented([label for label, _ in PLATE_STYLES])
        c.field("Фон плашки", self.lt_plate)
        self.lt_anim = Segmented([label for label, _ in PLATE_ANIMS])
        c.field("Анимация", self.lt_anim)
        self.lt_speed = SliderRow(self.s, "lt_anim_speed", "Скорость анимации", 0.5, 2.0, 0.1, "{:.1f}×",
                                  self._changed, "Меньше — плавнее и дольше, больше — быстрее")
        c.add(self.lt_speed)
        btns = QHBoxLayout()
        btns.setSpacing(8)
        self.lt_show = QPushButton("Показать сейчас")
        self.lt_preview = QPushButton("Превью")
        self.lt_preview.setToolTip("Открыть плашку в браузере — крутится по кругу, OBS не нужен")
        for b in (self.lt_show, self.lt_preview):
            b.setFont(theme.font("caps", 10))
            b.setCursor(Qt.PointingHandCursor)
            btns.addWidget(b, 1)
        c.add(btns)
        pl.addWidget(section("Плашка с именем", c, 6))

        # 07 watchdog
        c = Card()
        self.watchdog = Switch()
        c.row("Следить за камерой", self.watchdog, "Чёрный кадр, зависшая картинка, пропал сигнал")
        self.watchdog_sound = Switch()
        c.row("Звук тревоги", self.watchdog_sound)
        self.watchdog_switch = Switch()
        c.row("Переключить на запасную", self.watchdog_switch, "Сама уводит эфир со сломанной камеры")
        self.backup_cb = QComboBox()
        self.backup_row = c.field("Запасная сцена", self.backup_cb)
        pl.addWidget(section("Сторож камеры", c, 7))

        # 08 performance
        c = Card()
        self.use_gpu = Switch()
        c.row("Видеокарта", self.use_gpu, "DirectML: NVIDIA, AMD, Intel")
        self.detect_fps = self._spin(QSpinBox(), 2, 30, " /с")
        self.detect_fps.setToolTip("Как часто искать спикера. На слабых ноутбуках 6–10.")
        c.row("Распознавание", self.detect_fps, "Кадров нейросети в секунду")
        self.device_lbl = QLabel("—")
        theme.set_font(self.device_lbl, "mono", 11)
        self.device_lbl.setObjectName("muted")
        c.row("Устройство", self.device_lbl)
        pl.addWidget(section("Производительность", c, 8))
        pl.addStretch(1)

        scroll = QScrollArea()
        scroll.setWidget(content)
        scroll.setWidgetResizable(True)
        scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        for combo in (self.source_cb, self.backup_cb):
            combo.setSizeAdjustPolicy(QComboBox.AdjustToMinimumContentsLengthWithIcon)
            combo.setMinimumContentsLength(12)
            combo.setCursor(Qt.PointingHandCursor)
        outer.addWidget(scroll, 1)
        self._scroll = scroll
        return panel

    def _build_footer(self) -> QFrame:
        bar = QFrame()
        bar.setObjectName("footer")
        bar.setFixedHeight(30)
        h = QHBoxLayout(bar)
        h.setContentsMargins(20, 0, 16, 0)
        h.setSpacing(16)
        self.status_lbl = QLabel()
        theme.set_font(self.status_lbl, "caps", 10)
        h.addWidget(self.status_lbl, 1)
        self.footer_device = QLabel("")
        self.footer_device.setObjectName("faint")
        theme.set_font(self.footer_device, "mono", 10)
        h.addWidget(self.footer_device)
        ver = QLabel(f"v{__version__}")
        ver.setObjectName("faint")
        theme.set_font(ver, "mono", 10)
        h.addWidget(ver)
        self._status("Готово")
        return bar

    # ------------------------------------------------------------------ wiring

    def _wire(self):
        e = self.engine
        e.preview.connect(self._on_preview)
        e.status.connect(self._status)
        e.error.connect(self._on_error)
        e.running_changed.connect(self._on_running)
        e.active_changed.connect(self._on_active)
        e.device.connect(self._on_device)
        e.camera_fault.connect(self._on_fault)
        e.speaker_on_air.connect(self._show_plate)
        self.plate_failed.connect(lambda t: self._status(t, 6000, theme.DANGER))

        self.start_btn.clicked.connect(self.toggle_running)
        self.wide_btn.toggled.connect(e.set_paused)
        self.retarget_btn.clicked.connect(e.retarget)
        self.zone_btn.toggled.connect(self.preview.set_zone_mode)
        self.zone_reset_btn.clicked.connect(self._reset_zone)
        self.plate_btn.clicked.connect(lambda: self._show_plate(None))
        self.lt_show.clicked.connect(lambda: self._show_plate(None))
        self.conn_btn.clicked.connect(self._connection_dialog)
        self.refresh_btn.clicked.connect(self.refresh_obs)

        self.preview.clicked.connect(self._on_preview_click)
        self.preview.zone_drawn.connect(self._on_zone)

        self.source_cb.currentTextChanged.connect(self._on_source)
        self.shot_seg.changed.connect(self._on_shot)
        self.cut_seg.changed.connect(lambda i: self._set("cut_wide_shot", WIDE_SHOTS[i][1]))
        self.fixed_zoom.valueChanged.connect(lambda v: self._set("fixed_zoom", v))
        self.auto_cut.toggled.connect(self._on_auto_cut)
        self.reid.toggled.connect(lambda v: self._set("reid", v))
        self.wide_on_lost.toggled.connect(lambda v: self._set("wide_on_lost", v))
        self.lost_timeout.valueChanged.connect(lambda v: self._set("lost_timeout", v))
        self.lt_name.textChanged.connect(lambda v: self._set("lt_name", v))
        self.lt_title.textChanged.connect(lambda v: self._set("lt_title", v))
        self.lt_auto.toggled.connect(lambda v: self._set("lt_auto", v))
        self.lt_color.changed.connect(lambda v: self._set("lt_color", v))
        self.lt_plate.changed.connect(lambda i: self._set("lt_plate", PLATE_STYLES[i][1]))
        self.lt_anim.changed.connect(lambda i: self._set("lt_anim", PLATE_ANIMS[i][1]))
        self.lt_preview.clicked.connect(self._preview_plate)
        self.watchdog.toggled.connect(self._on_watchdog)
        self.watchdog_sound.toggled.connect(lambda v: self._set("watchdog_sound", v))
        self.watchdog_switch.toggled.connect(self._on_watchdog_switch)
        self.backup_cb.currentTextChanged.connect(lambda v: v and self._set("backup_scene", v))
        self.use_gpu.toggled.connect(lambda v: self._set("use_gpu", v))
        self.detect_fps.valueChanged.connect(lambda v: self._set("detect_fps", v))

        QShortcut(QKeySequence("F5"), self, activated=self.toggle_running)
        QShortcut(QKeySequence("Space"), self, activated=self.wide_btn.toggle)
        # N only while the preview has focus, so typing a name never fires it
        QShortcut(QKeySequence("N"), self.preview, activated=lambda: self._show_plate(None),
                  context=Qt.WidgetShortcut)

    def _load_into_ui(self):
        s = self.s
        idx = 4 if s.zoom_mode == "fixed" else next(
            (i for i, c in enumerate(SHOT_CHOICES) if c[2] == s.shot), 1)
        self.shot_seg.set_index(idx)
        self.fixed_zoom.setValue(s.fixed_zoom)
        self.fixed_zoom_row.setEnabled(s.zoom_mode == "fixed")
        self.auto_cut.setChecked(s.auto_cut)
        self.cut_seg.set_index(next((i for i, (_, k) in enumerate(WIDE_SHOTS) if k == s.cut_wide_shot), 1))
        self._on_auto_cut(s.auto_cut, save=False)
        self.reid.setChecked(s.reid)
        self.wide_on_lost.setChecked(s.wide_on_lost)
        self.lost_timeout.setValue(s.lost_timeout)
        self.lt_name.setText(s.lt_name)
        self.lt_title.setText(s.lt_title)
        self.lt_auto.setChecked(s.lt_auto)
        self.lt_color.set_color(s.lt_color)
        self.lt_plate.set_index(next((i for i, (_, k) in enumerate(PLATE_STYLES) if k == s.lt_plate), 0))
        self.lt_anim.set_index(next((i for i, (_, k) in enumerate(PLATE_ANIMS) if k == s.lt_anim), 0))
        self.watchdog.setChecked(s.watchdog)
        self.watchdog_sound.setChecked(s.watchdog_sound)
        self.watchdog_switch.setChecked(s.watchdog_switch)
        if s.backup_scene:
            self.backup_cb.addItem(s.backup_scene)
        self._on_watchdog(s.watchdog, save=False)
        self.use_gpu.setChecked(s.use_gpu)
        self.detect_fps.setValue(s.detect_fps)
        if s.source:
            self.source_cb.addItem(s.source)
        self._fill_modes()
        self._update_source_meta()

    # ------------------------------------------------------------------ helpers

    def _set(self, attr, value):
        setattr(self.s, attr, value)
        self._changed()

    def _changed(self):
        self._save_timer.start()

    def _preview_plate(self):
        """Looping preview of the plate in the default browser, with the current look."""
        try:
            page = write_page(self.s, preview=True)
        except Exception as e:
            self._status(f"Не удалось открыть превью: {e}", 6000, theme.DANGER)
            return
        QDesktopServices.openUrl(QUrl.fromLocalFile(str(page)))

    def _save(self):
        try:
            self.s.save()
        except Exception as e:
            log.warning("Не удалось сохранить настройки: %s", e)

    def _status(self, text: str, timeout_ms: int = 0, color: str | None = None):
        """Footer status: ● TEXT. With a timeout, falls back to the idle text afterwards."""
        dot = color or (theme.ACCENT if self.engine.running else theme.TEXT_FAINT)
        self.status_lbl.setText(f"<span style='color:{dot}'>●</span>&nbsp; "
                                f"<span style='color:{theme.TEXT_2}'>{text}</span>")
        if timeout_ms:
            self._status_timer.start(timeout_ms)
        else:
            self._idle_status = text

    def _set_air_label(self, live: bool):
        dot = theme.LIVE if live else theme.TEXT_FAINT
        self.air_label.setText(f"<span style='color:{dot}'>●</span>&nbsp; "
                               f"<span style='color:{theme.TEXT if live else theme.TEXT_MUTED}'>ЭФИР</span>")

    def _update_source_meta(self):
        n = len(self.s.modes())
        scenes = f"{n} сцен{'а' if n == 1 else 'ы' if 1 < n < 5 else ''}" if n else "сцены не выбраны"
        self.source_meta.setText("  ·  ".join(p for p in (self.s.source, scenes) if p))

    # ------------------------------------------------------------------ OBS lists

    def refresh_obs(self):
        try:
            c = ObsConnection(self.s.obs_host, self.s.obs_port, self.s.obs_password, timeout=3)
            version = c.version()
            scenes = c.scenes()
            self._scene_sources = {sc: c.scene_sources(sc) for sc in scenes}
            c.close()
        except Exception as e:
            self._set_obs_status(False, str(e) or e.__class__.__name__)
            return
        self._scenes = scenes
        self._set_obs_status(True, version)

        # every source that sits in some scene, in scene order, without the name plate
        sources = []
        for sc in scenes:
            for src in self._scene_sources[sc]:
                if src not in sources and not src.startswith("В кадре ·"):
                    sources.append(src)
        self.source_cb.blockSignals(True)
        self.source_cb.clear()
        self.source_cb.addItems(sources)
        if self.s.source in sources:
            self.source_cb.setCurrentText(self.s.source)
        elif sources:
            self.source_cb.setCurrentIndex(0)
            self.s.source = sources[0]
        self.source_cb.blockSignals(False)

        self.backup_cb.blockSignals(True)
        self.backup_cb.clear()
        self.backup_cb.addItems(scenes)
        self.backup_cb.setCurrentText(self.s.backup_scene) if self.s.backup_scene in scenes \
            else self.backup_cb.setCurrentIndex(-1)
        self.backup_cb.blockSignals(False)

        self._fill_modes()
        self._update_source_meta()

    def _fill_modes(self):
        """One row per scene that contains the camera (or the saved ones before OBS is reached)."""
        while self.modes_lay.count():
            w = self.modes_lay.takeAt(0).widget()
            if w:
                w.hide()
                w.setParent(None)
                w.deleteLater()
        modes = self.s.modes()
        if self._scene_sources:
            scenes = [sc for sc in self._scenes if self.s.source in self._scene_sources.get(sc, [])]
        else:
            scenes = list(modes)
        if not scenes:
            lbl = QLabel("Подключитесь к OBS и выберите камеру" if not self._scene_sources
                         else "Этой камеры нет ни в одной сцене")
            lbl.setObjectName("faint")
            theme.set_font(lbl, "meta", 12)
            self.modes_lay.addWidget(lbl)
            return
        for sc in scenes:
            self.modes_lay.addWidget(SceneModeRow(sc, modes.get(sc), self._on_scene_mode))

    def _set_obs_status(self, ok: bool, text: str):
        if ok:
            self.obs_pill.set(f"OBS {text}", theme.ACCENT)
            self.obs_pill.setToolTip("")
        else:
            self.obs_pill.set("OBS · нет связи", theme.DANGER)
            self.obs_pill.setToolTip(text)
            self._status("Нет связи с OBS — запущен ли OBS и включён ли WebSocket?", 8000, theme.DANGER)
            self.status_lbl.setToolTip(text)

    # ------------------------------------------------------------------ slots

    def toggle_running(self):
        if self.engine.running:
            self.engine.stop()
        else:
            self._save()
            self.preview.set_message("Запуск\nЗагружаю нейросеть и подключаюсь к OBS…")
            self.engine.start()

    def _on_running(self, running: bool):
        self.start_btn.setText(" Стоп" if running else " Старт")
        self.start_btn.setIcon(theme.icon("stop" if running else "play",
                                          "white" if running else theme.ON_ACCENT, size=14))
        self.start_btn.setProperty("running", running)
        self.start_btn.style().unpolish(self.start_btn)
        self.start_btn.style().polish(self.start_btn)
        for w in (self.source_cb, self.refresh_btn, self.use_gpu, self.conn_btn, self.modes_box):
            w.setEnabled(not running)
        if not running:
            self.preview.clear()
            self.preview.set_message(IDLE_MESSAGE)
            self.air.clear()
            self.air_meta.setText("")
            self._set_air_label(False)
            self._on_fault("")
            self._status("Остановлено")

    def _on_active(self, active: bool):
        self.preview.set_message(None if active else "Ни одна из отмеченных сцен не в эфире — слежение ждёт")

    def _on_preview(self, data):
        self.preview.set_data(data)
        self.air.set_data(data, self.preview.qimg)
        self._set_air_label(data.active and not data.paused and not data.fault)
        cut = {"close": "КРУПНЫЙ · ", "wide": "СРЕДНИЙ · "}.get(data.cut, "")
        self.air_meta.setText(f"{cut}{data.zoom:.2f}×")

    def _on_device(self, text: str):
        self.device_lbl.setText(text)
        self.footer_device.setText(text.upper())

    def _on_fault(self, text: str):
        if text == self._fault:
            return
        self._fault = text
        self.preview.set_fault(text)
        if text:
            self._status(text, 0, theme.DANGER)
            if self.s.watchdog_sound:
                self._beep()
                self._beep_timer.start()
        else:
            self._beep_timer.stop()
            if self.engine.running:
                self._status("Камера снова в порядке", 5000)

    def _beep(self):
        try:
            import winsound
            winsound.MessageBeep(winsound.MB_ICONHAND)
        except Exception:
            from PySide6.QtWidgets import QApplication
            QApplication.beep()

    def _show_plate(self, scene):
        if not self.s.lt_name.strip():
            self._status("Впишите имя для плашки в настройках (06 · Плашка с именем)", 5000, theme.WARN)
            return
        scene = scene or self.engine.active_scene
        if self.plate.show(scene, on_error=self.plate_failed.emit):
            self._status(f"Плашка: {self.s.lt_name.strip()}", int(anim_seconds(self.s) * 1000))

    def _on_error(self, text: str):
        self._status(f"Ошибка: {text}", 0, theme.DANGER)
        QMessageBox.warning(self, __app_name__, text)

    def _on_preview_click(self, x, y):
        if self.engine.running and self.engine.lock_at(x, y):
            self.wide_btn.setChecked(False)
            second = self.engine.mode == "duo" and self.engine.partner_id is not None
            self._status("Второй человек выбран" if second else "Цель выбрана", 2000)

    def _on_zone(self, x0, y0, x1, y1):
        self._set("zone", [round(x0, 3), round(y0, 3), round(x1, 3), round(y1, 3)])
        self.zone_btn.setChecked(False)
        self.engine.retarget()

    def _reset_zone(self):
        self._set("zone", [0.0, 0.0, 1.0, 1.0])

    def _on_source(self, name):
        if name:
            self._set("source", name)
            self._fill_modes()
            self._update_source_meta()

    def _on_scene_mode(self, scene: str, mode: str | None):
        modes = self.s.modes()
        if mode:
            modes[scene] = mode
        else:
            modes.pop(scene, None)
        self.s.scene_modes = modes
        self.s.active_scenes = []
        self.s.scene = next((sc for sc, m in modes.items() if m != "wide"), next(iter(modes), ""))
        self._changed()
        self._update_source_meta()

    def _on_shot(self, i):
        _, mode, shot = SHOT_CHOICES[i]
        self.s.zoom_mode = mode
        if shot:
            self.s.shot = shot
        self.fixed_zoom_row.setEnabled(mode == "fixed")
        self._changed()

    def _on_auto_cut(self, on: bool, save=True):
        self.cut_seg_row.setEnabled(on)
        self.cut_hold_row.setEnabled(on)
        if save:
            self._set("auto_cut", on)

    def _on_watchdog(self, on: bool, save=True):
        for w in (self.watchdog_sound, self.watchdog_switch):
            w.setEnabled(on)
        self.backup_row.setEnabled(on and self.s.watchdog_switch)
        if save:
            self._set("watchdog", on)

    def _on_watchdog_switch(self, on: bool):
        self.backup_row.setEnabled(on and self.s.watchdog)
        self._set("watchdog_switch", on)

    def _connection_dialog(self):
        if ConnectionDialog(self.s, self).exec():
            self._save()
            self.refresh_obs()

    def closeEvent(self, e):
        self.engine.stop()
        self._save()
        super().closeEvent(e)

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (QDialog, QFrame, QHBoxLayout, QLabel, QLineEdit, QPushButton, QSpinBox,
                               QVBoxLayout)

from speakertrack.obs_client import ObsConnection
from speakertrack.ui import theme
from speakertrack.ui.widgets import Card, Switch, caps_label


class ConnectionDialog(QDialog):
    def __init__(self, settings, parent=None):
        super().__init__(parent)
        self.s = settings
        self.setWindowTitle("Подключение к OBS")
        self.setMinimumWidth(440)

        accent = QFrame()
        accent.setFixedHeight(2)
        accent.setStyleSheet(f"background: {theme.ACCENT};")

        title = QLabel("Подключение")
        theme.set_font(title, "display", 18)
        hint = QLabel("В OBS: Инструменты → Настройки сервера WebSocket → включить. "
                      "Порт и пароль берите оттуда же.")
        hint.setObjectName("muted")
        hint.setWordWrap(True)
        theme.set_font(hint, "meta", 12)

        self.host = QLineEdit(settings.obs_host)
        self.host.setPlaceholderText("localhost")
        self.port = QSpinBox()
        self.port.setRange(1, 65535)
        self.port.setValue(settings.obs_port)
        self.port.setFixedWidth(110)
        self.password = QLineEdit(settings.obs_password)
        self.password.setEchoMode(QLineEdit.Password)
        self.password.setPlaceholderText("без пароля")
        self.auto = Switch()
        self.auto.setChecked(settings.auto_connect)

        addr = QHBoxLayout()
        addr.setSpacing(8)
        for label, w, stretch in (("Адрес", self.host, 1), ("Порт", self.port, 0)):
            col = QVBoxLayout()
            col.setSpacing(6)
            col.addWidget(caps_label(label))
            col.addWidget(w)
            addr.addLayout(col, stretch)
        pw = QVBoxLayout()
        pw.setSpacing(6)
        pw.addWidget(caps_label("Пароль"))
        pw.addWidget(self.password)

        card = Card()
        card.row("Подключаться при запуске", self.auto, "Сразу загружать сцены из OBS")

        self.result_label = QLabel("")
        self.result_label.setWordWrap(True)
        theme.set_font(self.result_label, "body", 12)

        test = QPushButton("Проверить")
        test.setFont(theme.font("caps", 10))
        test.setCursor(Qt.PointingHandCursor)
        test.clicked.connect(self._test)
        cancel = QPushButton("Отмена")
        cancel.setFont(theme.font("caps", 10))
        cancel.setCursor(Qt.PointingHandCursor)
        cancel.clicked.connect(self.reject)
        ok = QPushButton("Сохранить")
        ok.setObjectName("primary")
        ok.setFont(theme.font("button", 11))
        ok.setMinimumHeight(36)
        ok.setCursor(Qt.PointingHandCursor)
        ok.setDefault(True)
        ok.clicked.connect(self._accept)
        buttons = QHBoxLayout()
        buttons.addWidget(test)
        buttons.addStretch(1)
        buttons.addWidget(cancel)
        buttons.addWidget(ok)

        body = QVBoxLayout()
        body.setContentsMargins(24, 20, 24, 22)
        body.setSpacing(14)
        body.addWidget(caps_label("OBS WebSocket", "eyebrow", "eyebrow"))
        body.addWidget(title)
        body.addWidget(hint)
        body.addSpacing(4)
        body.addLayout(addr)
        body.addLayout(pw)
        body.addWidget(card)
        body.addWidget(self.result_label)
        body.addLayout(buttons)

        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(0)
        lay.addWidget(accent)
        lay.addLayout(body)

    def _result(self, ok: bool | None, text: str):
        color = theme.TEXT_MUTED if ok is None else (theme.ACCENT if ok else theme.DANGER)
        self.result_label.setText(f"<span style='color:{color}'>●</span>&nbsp; {text}")

    def _test(self):
        self._result(None, "Проверяю…")
        self.result_label.repaint()
        try:
            c = ObsConnection(self.host.text().strip() or "localhost", self.port.value(), self.password.text(),
                              timeout=3)
            v = c.version()
            c.close()
            self._result(True, f"Подключено: {v}")
        except Exception as e:
            self._result(False, f"Не удалось подключиться: {e or e.__class__.__name__}")

    def _accept(self):
        self.s.obs_host = self.host.text().strip() or "localhost"
        self.s.obs_port = self.port.value()
        self.s.obs_password = self.password.text()
        self.s.auto_connect = self.auto.isChecked()
        self.accept()

import logging
import sys
from logging.handlers import RotatingFileHandler

from PySide6.QtWidgets import QApplication

from speakertrack import __app_name__, __version__
from speakertrack.settings import Settings, app_data_dir
from speakertrack.ui import theme
from speakertrack.ui.main_window import MainWindow
from speakertrack.ui.widgets import WheelGuard


def setup_logging():
    handlers = [RotatingFileHandler(app_data_dir() / "speakertrack.log", maxBytes=1_000_000,
                                    backupCount=2, encoding="utf-8")]
    if sys.stderr:
        handlers.append(logging.StreamHandler())
    logging.basicConfig(level=logging.INFO, handlers=handlers,
                        format="%(asctime)s %(levelname)s %(name)s: %(message)s")
    logging.getLogger(__name__).info("%s %s starting", __app_name__, __version__)


def selftest() -> int:
    """`VKadre.exe --selftest`: checks the bundled model and GPU, result goes to the log."""
    import numpy as np
    from speakertrack.detector import PoseDetector
    log = logging.getLogger("selftest")
    s = Settings.load()
    try:
        d = PoseDetector(s.model_path(), use_gpu=s.use_gpu)
        d.detect(np.zeros((360, 640, 3), np.uint8))
        log.info("SELFTEST OK: model %s, device %s", s.model_path().name, d.device_name)
        return 0
    except Exception:
        log.exception("SELFTEST FAILED")
        return 1


def main():
    setup_logging()
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    app = QApplication(sys.argv)
    app.setApplicationName(__app_name__)
    app.setApplicationVersion(__version__)
    theme.apply(app)
    app.installEventFilter(WheelGuard(app))
    win = MainWindow(Settings.load())
    win.show()
    sys.exit(app.exec())


if __name__ == "__main__":
    main()

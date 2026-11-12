# PyInstaller build: pyinstaller SpeakerTrack.spec  ->  dist/VKadre/VKadre.exe
from PyInstaller.utils.hooks import collect_dynamic_libs

a = Analysis(
    ["speakertrack/__main__.py"],
    pathex=["."],
    binaries=collect_dynamic_libs("onnxruntime"),
    datas=[("models/*.onnx", "models"), ("LICENSE", "."),
           ("speakertrack/ui/fonts/*", "speakertrack/ui/fonts"),
           ("speakertrack/overlay/*.html", "speakertrack/overlay")],
    hiddenimports=["obsws_python", "PySide6.QtSvg"],
    excludes=["tkinter", "matplotlib", "torch", "cv2",
              "PySide6.QtQml", "PySide6.QtQuick", "PySide6.QtNetwork", "PySide6.QtPdf",
              "PySide6.QtWebEngineCore", "PySide6.Qt3DCore", "PySide6.QtMultimedia"],
)

# Drop Qt pieces the app never loads (software OpenGL, QML/Quick, PDF, translations).
_drop = ("opengl32sw", "Qt6Quick", "Qt6Qml", "Qt6Pdf", "Qt6Network", "Qt6VirtualKeyboard",
         "qpdf", "translations")
a.binaries = [b for b in a.binaries if not any(d.lower() in b[0].lower() for d in _drop)]
a.datas = [d for d in a.datas if "translations" not in d[0].lower()]

pyz = PYZ(a.pure)
exe = EXE(
    pyz, a.scripts, [],
    exclude_binaries=True,
    name="VKadre",
    console=False,
    upx=False,
)
coll = COLLECT(exe, a.binaries, a.datas, name="VKadre", upx=False)

"""
Exports a YOLO11 pose model to ONNX into models/.

Needs a separate dev environment (it pulls PyTorch):
    pip install torch torchvision --index-url https://download.pytorch.org/whl/cpu
    pip install ultralytics onnx onnxslim
    python tools/export_model.py            # yolo11n-pose, 640
    python tools/export_model.py yolo11s-pose 640
"""

import shutil
import sys
from pathlib import Path

from ultralytics import YOLO

name = sys.argv[1] if len(sys.argv) > 1 else "yolo11n-pose"
size = int(sys.argv[2]) if len(sys.argv) > 2 else 640

root = Path(__file__).resolve().parent.parent
out_dir = root / "models"
out_dir.mkdir(exist_ok=True)

model = YOLO(f"{name}.pt")
onnx_path = Path(model.export(format="onnx", imgsz=size, opset=17, simplify=True, dynamic=False))
target = out_dir / f"{name}.onnx" if size == 640 else out_dir / f"{name}-{size}.onnx"
shutil.move(str(onnx_path), target)
print("Saved:", target)

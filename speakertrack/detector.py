"""
Person + pose detector on ONNX Runtime (YOLO11-pose exported to ONNX).

Runs on the GPU through DirectML (any DX12 GPU: NVIDIA / AMD / Intel), falls back to CPU.
Model output: (1, 56, N) — 4 box (cx, cy, w, h in input pixels), 1 person score,
17 keypoints * (x, y, conf). Scores are already sigmoid-ed by the export.
"""

from __future__ import annotations

import logging
from pathlib import Path

import numpy as np
import onnxruntime as ort

from speakertrack.framing import Person

log = logging.getLogger(__name__)


def nms(boxes: np.ndarray, scores: np.ndarray, iou_thr: float) -> list[int]:
    order = scores.argsort()[::-1]
    keep = []
    while order.size:
        i = order[0]
        keep.append(int(i))
        if order.size == 1:
            break
        rest = order[1:]
        xx0 = np.maximum(boxes[i, 0], boxes[rest, 0])
        yy0 = np.maximum(boxes[i, 1], boxes[rest, 1])
        xx1 = np.minimum(boxes[i, 2], boxes[rest, 2])
        yy1 = np.minimum(boxes[i, 3], boxes[rest, 3])
        inter = np.clip(xx1 - xx0, 0, None) * np.clip(yy1 - yy0, 0, None)
        area_i = (boxes[i, 2] - boxes[i, 0]) * (boxes[i, 3] - boxes[i, 1])
        area_r = (boxes[rest, 2] - boxes[rest, 0]) * (boxes[rest, 3] - boxes[rest, 1])
        iou = inter / np.maximum(area_i + area_r - inter, 1e-9)
        order = rest[iou <= iou_thr]
    return keep


def resize_bilinear(img: np.ndarray, nw: int, nh: int) -> np.ndarray:
    """Small numpy bilinear resize (the engine asks OBS for the right size, so this is rarely hit)."""
    h, w = img.shape[:2]
    if (w, h) == (nw, nh):
        return img
    xs = (np.arange(nw) + 0.5) * w / nw - 0.5
    ys = (np.arange(nh) + 0.5) * h / nh - 0.5
    x0 = np.clip(np.floor(xs).astype(int), 0, w - 1)
    y0 = np.clip(np.floor(ys).astype(int), 0, h - 1)
    x1, y1 = np.clip(x0 + 1, 0, w - 1), np.clip(y0 + 1, 0, h - 1)
    fx = np.clip(xs - x0, 0, 1)[None, :, None]
    fy = np.clip(ys - y0, 0, 1)[:, None, None]
    f = img.astype(np.float32)
    top = f[y0][:, x0] * (1 - fx) + f[y0][:, x1] * fx
    bot = f[y1][:, x0] * (1 - fx) + f[y1][:, x1] * fx
    return (top * (1 - fy) + bot * fy).astype(np.uint8)


def letterbox(img: np.ndarray, size: int):
    """Resize keeping aspect, pad to size x size. Returns (padded image, scale, pad_x, pad_y)."""
    h, w = img.shape[:2]
    scale = min(size / w, size / h)
    nw, nh = int(round(w * scale)), int(round(h * scale))
    resized = resize_bilinear(img, nw, nh)
    out = np.full((size, size, 3), 114, dtype=np.uint8)
    px, py = (size - nw) // 2, (size - nh) // 2
    out[py:py + nh, px:px + nw] = resized
    return out, scale, px, py


def decode(output: np.ndarray, img_w: int, img_h: int, scale: float, px: int, py: int,
           conf_thr: float, iou_thr: float = 0.5) -> list[Person]:
    """Turns raw (1, 56, N) model output into normalized Person objects."""
    pred = output[0].T                      # (N, 56)
    scores = pred[:, 4]
    m = scores >= conf_thr
    if not np.any(m):
        return []
    pred, scores = pred[m], scores[m]

    cx, cy, bw, bh = pred[:, 0], pred[:, 1], pred[:, 2], pred[:, 3]
    boxes = np.stack([cx - bw / 2, cy - bh / 2, cx + bw / 2, cy + bh / 2], axis=1)
    keep = nms(boxes, scores, iou_thr)

    people = []
    for i in keep:
        b = boxes[i]
        x0 = (b[0] - px) / scale / img_w
        y0 = (b[1] - py) / scale / img_h
        x1 = (b[2] - px) / scale / img_w
        y1 = (b[3] - py) / scale / img_h
        k = pred[i, 5:].reshape(17, 3)
        kxy = np.stack([(k[:, 0] - px) / scale / img_w, (k[:, 1] - py) / scale / img_h], axis=1)
        people.append(Person(box=(float(x0), float(y0), float(x1), float(y1)),
                             score=float(scores[i]), kxy=kxy, kconf=k[:, 2].copy()))
    return people


class PoseDetector:
    def __init__(self, model_path: str | Path, use_gpu: bool = True):
        model_path = Path(model_path)
        if not model_path.exists():
            raise FileNotFoundError(f"Модель не найдена: {model_path}")

        available = ort.get_available_providers()
        providers = []
        if use_gpu:
            for p in ("CUDAExecutionProvider", "DmlExecutionProvider"):
                if p in available:
                    providers.append(p)
        providers.append("CPUExecutionProvider")

        opts = ort.SessionOptions()
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        if "DmlExecutionProvider" in providers:
            # DirectML requirements
            opts.enable_mem_pattern = False
            opts.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL

        self.session = ort.InferenceSession(str(model_path), sess_options=opts, providers=providers)
        self.provider = self.session.get_providers()[0]
        inp = self.session.get_inputs()[0]
        self.input_name = inp.name
        shape = inp.shape
        self.size = shape[2] if isinstance(shape[2], int) else 640
        log.info("Detector: %s, input %s, provider %s", model_path.name, self.size, self.provider)

    @property
    def device_name(self) -> str:
        return {
            "CUDAExecutionProvider": "GPU (CUDA)",
            "DmlExecutionProvider": "GPU (DirectML)",
        }.get(self.provider, "CPU")

    def detect(self, img_rgb: np.ndarray, conf_thr: float = 0.4) -> list[Person]:
        ih, iw = img_rgb.shape[:2]
        lb, scale, px, py = letterbox(img_rgb, self.size)
        x = np.ascontiguousarray(lb.transpose(2, 0, 1)[None], dtype=np.float32) / 255.0
        out = self.session.run(None, {self.input_name: x})[0]
        return decode(out, iw, ih, scale, px, py, conf_thr)

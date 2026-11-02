"""Cuts the kill-feed region out of Valorant clips, at 5 Hz, for the
`real_valorant_recordings` replay test in src/valorant.rs.

    python extract.py "E:/Bureau/20261101_22*.mp4" ../../../../../.test-tmp/valo2

Needs numpy and opencv-python. Writes one <clip>.npy per clip: frames x 270
x 576 x 3, BGR, the 1080p crop of valorant::REGION (x 1344-1920, y 75-345).
"""
import glob
import os
import sys

import cv2
import numpy as np

HZ = 5

pattern, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
for mp4 in sorted(glob.glob(pattern)):
    name = os.path.splitext(os.path.basename(mp4))[0]
    cap = cv2.VideoCapture(mp4)
    step = cap.get(cv2.CAP_PROP_FPS) / HZ
    feed, i, wanted = [], 0, 0.0
    while cap.grab():
        if i >= wanted:
            ok, frame = cap.retrieve()
            if not ok:
                break
            feed.append(frame[75:345, 1344:1920].copy())
            wanted += step
        i += 1
    np.save(os.path.join(out, name + ".npy"), np.stack(feed))
    print(name, len(feed))

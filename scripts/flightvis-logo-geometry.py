#!/usr/bin/env python3
"""Owner-approved flight_vis logo geometry (reference: flightvis-logo-final.py)."""

from __future__ import annotations

import math

CANVAS = 1024
S = 1.12
T = (610.0, 425.0)
TAIL_SRC = (672.0, 450.0)
TIP_SRC = (982.0, 118.0)

AXIS = (TIP_SRC[0] - TAIL_SRC[0], TIP_SRC[1] - TAIL_SRC[1])
U = (AXIS[0] / math.hypot(*AXIS), AXIS[1] / math.hypot(*AXIS))
P = (T[0] - 60 * U[0], T[1] - 60 * U[1])
C2 = (P[0] - 100 * U[0], P[1] - 100 * U[1])

TRACK_D = (
    "M 150,885 "
    "C 330,895 590,850 640,740 "
    "C 680,655 660,640 590,640 "
    "C 400,640 212,630 212,520 "
    f"C 205,195 {C2[0]:.2f},{C2[1]:.2f} {P[0]:.2f},{P[1]:.2f} "
    f"L {T[0]:.0f},{T[1]:.0f}"
)

TRACK_STROKE = 26
TRACK_DASH = "46 24"
TRACK_COLOR = "#FF00CC"

MAP_RX = 56
MAP_RECT = (72, 88, 880, 848)

OCEAN = "#1E90FF"
LAND = "#6DB33F"
BORDER = "#0D3A8C"


def tip_at() -> tuple[float, float]:
    return (T[0] + S * AXIS[0], T[1] + S * AXIS[1])


def plane_transform() -> str:
    return f"translate({T[0]} {T[1]}) scale({S}) translate({-TAIL_SRC[0]} {-TAIL_SRC[1]})"


if __name__ == "__main__":
    print(f"canvas={CANVAS}")
    print(f"plane_angle_deg={math.degrees(math.atan2(AXIS[1], AXIS[0])):.2f}")
    print(f"tail_at={T}")
    print(f"tip_at=({tip_at()[0]:.1f}, {tip_at()[1]:.1f})")
    print(f"P={P}")
    print(f"c2={C2}")
    print(TRACK_D)
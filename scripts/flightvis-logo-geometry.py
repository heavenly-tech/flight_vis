#!/usr/bin/env python3
"""Owner-approved flight_vis logo geometry (reference: flightvis-logo-final.py)."""

from __future__ import annotations

import math

CANVAS = 1024
S = 1.12
TAIL_AT = (610, 425)
TAIL_SRC = (672, 450)
TIP_SRC = (982, 118)

AXIS = (TIP_SRC[0] - TAIL_SRC[0], TIP_SRC[1] - TAIL_SRC[1])
PLANE_ANGLE_RAD = math.atan2(AXIS[1], AXIS[0])
APPROACH_UNIT = (-AXIS[0] / math.hypot(*AXIS), -AXIS[1] / math.hypot(*AXIS))

TRACK_D = (
    "M 150,885 "
    "C 330,895 590,850 640,740 "
    "C 680,655 660,640 590,640 "
    "C 400,640 212,630 212,520 "
    "C 198,285 205,195 248,155 "
    "C 325,118 445,98 575,108 "
    "C 720,118 860,168 935,248 "
    "C 975,295 945,370 875,408 "
    "C 820,432 555,592 474,571 "
    "L 610,425"
)

TRACK_STROKE = 26
TRACK_DASH = "46 24"
TRACK_COLOR = "#FF00CC"

MAP_RX = 56
MAP_RECT = (72, 88, 880, 848)

OCEAN = "#3A8FD4"
LAND = "#59B450"
GRID = "#1A3260"
GRID_OPACITY = 0.14
BORDER = "#152B52"


def tip_at() -> tuple[float, float]:
    return (TAIL_AT[0] + S * AXIS[0], TAIL_AT[1] + S * AXIS[1])


def plane_transform() -> str:
    return f"translate({TAIL_AT[0]} {TAIL_AT[1]}) scale({S}) translate({-TAIL_SRC[0]} {-TAIL_SRC[1]})"


def approach_point(dist: float) -> tuple[float, float]:
    return (
        TAIL_AT[0] + APPROACH_UNIT[0] * dist,
        TAIL_AT[1] + APPROACH_UNIT[1] * dist,
    )


if __name__ == "__main__":
    print(f"canvas={CANVAS}")
    print(f"plane_angle_deg={math.degrees(PLANE_ANGLE_RAD):.2f}")
    print(f"tail_at={TAIL_AT}")
    print(f"tip_at=({tip_at()[0]:.1f}, {tip_at()[1]:.1f})")
    print(f"approach_180={approach_point(180)}")
    print(TRACK_D)
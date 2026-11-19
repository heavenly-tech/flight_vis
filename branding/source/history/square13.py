#!/usr/bin/env python3
"""Historical iteration square13 (= approved flightvis-logo-final.py)."""

from pathlib import Path
import runpy

runpy.run_path(str(Path(__file__).resolve().parent.parent / "flightvis-logo-final.py"), run_name="__main__")
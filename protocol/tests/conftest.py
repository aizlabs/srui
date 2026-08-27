from __future__ import annotations

import sys
from pathlib import Path

PROTOCOL_DIR = Path(__file__).resolve().parent.parent
if str(PROTOCOL_DIR) not in sys.path:
    sys.path.insert(0, str(PROTOCOL_DIR))

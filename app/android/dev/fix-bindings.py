#!/usr/bin/env python3
"""UniFFI 0.32.2 Kotlin reserved-name collisions. No wire symbols or Rust APIs change."""
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
assert text.count('fun `close`()') == 2, 'Review UniFFI close collision after upgrading'
assert text.count('val `message`: kotlin.String') == 3, 'Review UniFFI error collision after upgrading'
# Backticked names are UniFFI's API fields; Throwable.message is not backticked.
text = text.replace('fun `close`()', 'fun `shutdown`()').replace('`message`', '`detail`')
path.write_text(text)

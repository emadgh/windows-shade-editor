from pathlib import Path
import base64
import zlib

parts = sorted(Path("tools/upgrade_chunks").glob("*.txt"))
if not parts:
    raise SystemExit("No upgrade chunks found")
payload = "".join(path.read_text(encoding="ascii") for path in parts).encode("ascii")
source = zlib.decompress(base64.b85decode(payload))
Path("tools/apply_production_upgrade.py").write_bytes(source)
print(f"Decoded production upgrade script: {len(source)} bytes")

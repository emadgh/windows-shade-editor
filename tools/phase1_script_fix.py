from pathlib import Path
p = Path("tools/apply_phase1_hardening.py")
t = p.read_text(encoding="utf-8")
old = 'text = once(text, "            snapshot_code: &snapshot_code,", "            snapshot_name: &snapshot_name,\\n            test_code: &test_code,", "export preview context")'
new = 'text = first(text, "            snapshot_code: &snapshot_code,", "            snapshot_name: &snapshot_name,\\n            test_code: &test_code,", "export preview context")'
if t.count(old) != 1:
    raise RuntimeError(f"phase1 script patch expected one target, found {t.count(old)}")
p.write_text(t.replace(old, new, 1), encoding="utf-8")

from pathlib import Path

path = Path(__file__).with_name("apply_v0181_feedback.py")
text = path.read_text(encoding="utf-8")
old = '''module_at = export_rs.find("#[cfg(test)]\\nmod tests {")
if module_at < 0:
    raise RuntimeError("export tests module not found")
'''
new = '''module_at = export_rs.find("#[cfg(test)]\\nmod streaming_tests {")
if module_at < 0:
    raise RuntimeError("export streaming_tests module not found")
'''
if text.count(old) != 1:
    raise RuntimeError(f"expected one test-module patch target, found {text.count(old)}")
path.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
print("Fixed v0.18.1 patcher test module target")

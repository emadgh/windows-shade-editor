from pathlib import Path

p = Path("src/validation.rs")
s = p.read_text(encoding="utf-8")
old = '''        // Horizontal Predictor is a compression transform, not image semantics.
        // The image-tiff encoder's predictor stride excludes appended
        // ExtraSamples, so production exports intentionally normalize it off
        // for Spot/extra-channel TIFFs to guarantee pixel integrity.
        None
    };'''
new = '''        // Horizontal Predictor is a compression transform, not image semantics.
        // The image-tiff encoder's predictor stride excludes appended
        // ExtraSamples, so production exports intentionally normalize it off
        // for Spot/extra-channel TIFFs to guarantee pixel integrity. TIFF
        // Predictor value 1 explicitly means no prediction.
        Some(1)
    };'''
if old not in s:
    raise RuntimeError("predictor validator tag anchor not found")
p.write_text(s.replace(old, new, 1), encoding="utf-8")

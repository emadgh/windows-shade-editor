from pathlib import Path
import subprocess

path = Path("shell/ShadeEditorShell.cpp")
text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
anchor = '#include "ShadeProjectData.h"'
exports = '''#include "ShadeProjectData.h"

#pragma comment(linker, "/EXPORT:DllGetClassObject")
#pragma comment(linker, "/EXPORT:DllCanUnloadNow")'''
if '#pragma comment(linker, "/EXPORT:DllGetClassObject")' not in text:
    if anchor not in text:
        raise RuntimeError("ShadeProjectData include anchor not found")
    text = text.replace(anchor, exports, 1)
    path.write_text(text, encoding="utf-8", newline="\n")
subprocess.run(["git", "add", str(path)], check=True)

from pathlib import Path
import subprocess

# The immediately preceding commit contains the fully guarded patcher. Execute
# that exact reviewed script, then canonicalize the one appended test file so
# `git diff --check` sees exactly one trailing newline and no blank EOF line.
script = subprocess.check_output(
    ["git", "show", "HEAD^:.github/scripts/_temporary_apply_ui_followup.py"],
    text=True,
)
exec(compile(script, "_temporary_apply_ui_followup_previous.py", "exec"), {})

curve = Path("src/ui/curve_editor.rs")
curve.write_text(
    curve.read_text(encoding="utf-8").rstrip() + "\n",
    encoding="utf-8",
    newline="\n",
)
print("UI follow-up patch EOF canonicalized")

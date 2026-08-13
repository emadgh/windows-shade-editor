from pathlib import Path

p = Path("src/app_main.rs")
s = p.read_text(encoding="utf-8")
old = '''                    channel_display_name(
                        palette.as_ref(),
                        &channel_names[selected_index],
                        selected_index,
                    )
                };'''
new = '''                    channel_display_name(
                        palette.as_ref(),
                        &channel_names[selected_index],
                        selected_index,
                    )
                    .to_owned()
                };'''
if old not in s:
    raise RuntimeError("all-channel selected display anchor not found")
p.write_text(s.replace(old, new, 1), encoding="utf-8")

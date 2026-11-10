"""Render production registration contract, changing only fixture transport/addresses."""
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("renderer", ROOT / "tools/render_registration_kratos.py")
renderer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(renderer)
config = renderer.render(json.loads(renderer.TEMPLATE.read_text()),
                         "fixture-only-guard-secret-at-least-32-bytes",
                         "http://127.0.0.1:19481/registration/pre-persist")
config["serve"]["public"].update({"base_url": "http://127.0.0.1:19433/", "host": "127.0.0.1", "port": 19433})
config["serve"]["admin"].update({"base_url": "http://127.0.0.1:19434/", "host": "127.0.0.1", "port": 19434})
config["selfservice"]["methods"]["password"]["config"] = {"haveibeenpwned_enabled": False}
runtime = Path(__file__).resolve().parent / "runtime"
runtime.mkdir(exist_ok=True)
for name in ("guarded.json", "kratos.json"):
    (runtime / name).write_text(json.dumps(config, indent=2) + "\n")

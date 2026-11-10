#!/usr/bin/env python3
"""Render a secret-bearing runtime Kratos config without putting a key in argv/git."""
import argparse
import copy
import ipaddress
import json
import os
from pathlib import Path
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / "ory/kratos/kratos.yml"
BODY = "file:///etc/config/kratos/registration-hook.jsonnet"
PLACEHOLDER = "__EMUMET_REGISTRATION_GUARD_BEARER__"
NON_REGISTRATION = {"profile", "link", "totp", "lookup_secret"}


def validate_contract(config):
    """Adding a registration method requires matching pre-persist coverage first."""
    methods = config["selfservice"]["methods"]
    active = []
    for name, method in methods.items():
        if name in NON_REGISTRATION or not method.get("enabled", False):
            continue
        if name == "code" and not method.get("passwordless_enabled", False):
            continue
        active.append(name)
    if active != ["password"]:
        raise ValueError("only password registration is accepted by this deployment contract")
    hooks = config["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"]
    if len(hooks) != 1 or hooks[0]["hook"] != "web_hook":
        raise ValueError("registration requires the method-specific guard hook")
    hook = hooks[0]["config"]
    if (hook["method"] != "POST" or hook["body"] != BODY
            or hook["response"] != {"parse": True, "ignore": False}
            or hook["auth"] != {
                "type": "api_key",
                "config": {"name": "Authorization", "in": "header", "value": PLACEHOLDER},
            }):
        raise ValueError("registration guard contract changed")
    return hook


def validate_url(value):
    url = urlsplit(value)
    if (not url.hostname or url.username or url.password or url.query or url.fragment
            or url.path != "/registration/check"):
        raise ValueError("hook URL must name the private registration check endpoint")
    loopback = False
    try:
        loopback = ipaddress.ip_address(url.hostname).is_loopback
    except ValueError:
        pass
    if url.scheme != "https" and not (url.scheme == "http" and loopback):
        raise ValueError("non-loopback hook transport requires HTTPS")


def render(config, secret, hook_url):
    if len(secret) < 32 or not all(33 <= ord(c) <= 126 for c in secret):
        raise ValueError("hook secret must contain at least 32 visible ASCII bytes")
    validate_url(hook_url)
    config = copy.deepcopy(config)
    hook = validate_contract(config)
    hook["url"] = hook_url
    hook["auth"]["config"]["value"] = "Bearer " + secret
    return config


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--secret-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--hook-url", default="http://127.0.0.1:8081/registration/check")
    args = parser.parse_args()
    try:
        if args.secret_file.stat().st_mode & 0o077:
            raise ValueError("hook secret file must not be group/world-readable")
        secret = args.secret_file.read_text(encoding="ascii").strip()
        config = render(json.loads(TEMPLATE.read_text()), secret, args.hook_url)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        # Refuse overwriting another file; output contains a bearer credential.
        fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as output:
            json.dump(config, output, indent=2)
            output.write("\n")
    except (ValueError, OSError, KeyError, UnicodeError):
        parser.exit(1, "Could not render guarded Kratos config; check contract, key file and output path.\n")
    print("Rendered guarded Kratos config (secret omitted).")


if __name__ == "__main__":
    main()

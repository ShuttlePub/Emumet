"""Run against disposable real Kratos, recording only synthetic/redacted evidence."""
import argparse
import base64
import copy
from email import policy
from email.parser import Parser
import http.client
import http.cookiejar
import json
import os
from pathlib import Path
import re
import shutil
import socket
import ssl
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent
PUBLIC = "http://127.0.0.1:18433"
ADMIN = "http://127.0.0.1:18434"
STUB = "http://127.0.0.1:18445"
PASSWORD = "Fixture-only-Password-4281!"


def redact(value):
    if isinstance(value, list):
        return [redact(v) for v in value]
    if not isinstance(value, dict):
        return value
    out = {k: ("<redacted>" if k in (
        "session_token", "token", "csrf_token", "credentials", "password"
    ) else redact(v)) for k, v in value.items()}
    if out.get("name") == "csrf_token" and "value" in out:
        out["value"] = "<redacted>"
    return out


def client():
    return urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
    )


def mail_code(raw):
    message = Parser(policy=policy.default).parsestr(raw)
    part = message.get_body(preferencelist=("plain",))
    text = part.get_content() if part else message.get_content()
    matches = re.findall(r"\b[0-9]{6}\b", text)
    return matches[0] if matches else None


def request(url, body=None, opener=None, timeout=150, form=False):
    raw = None if body is None else (
        urllib.parse.urlencode(body).encode() if form else json.dumps(body).encode())
    req = urllib.request.Request(url, raw, {
        "Accept": "application/json", "Content-Type":
            "application/x-www-form-urlencoded" if form else "application/json",
    })
    started = time.monotonic()
    try:
        response = (opener or client()).open(req, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    except (http.client.RemoteDisconnected, TimeoutError, urllib.error.URLError) as error:
        return {"status": None, "seconds": round(time.monotonic() - started, 3),
                "body": None, "transport_error": type(error).__name__}
    text = response.read().decode()
    try:
        data = json.loads(text)
    except ValueError:
        data = text
    return {"status": response.code, "seconds": round(time.monotonic() - started, 3),
            "body": data}


class Suite:
    def __init__(self, args):
        self.args = args
        self.compose = [args.engine, "compose", "-p", "emumet-registration-guard-e15b",
                        "-f", str(ROOT / "compose.yml")]
        self.results = []
        self.started = False
        self.skipping = bool(args.skip_until)
        self.out = ROOT / "artifacts"
        self.out.mkdir(exist_ok=True)
        (self.out / "runtime").mkdir(exist_ok=True)
        suffix = "_" + uuid.uuid4().hex[:12] if args.fresh_databases else ""
        self.kratos_db = "kratos" + suffix
        self.app_db = "emumet" + suffix
        if suffix:
            dsn = "postgres://guard:fixture-only@postgres:5432/{}?sslmode=disable"
            override = self.out / "databases.json"
            override.write_text(json.dumps({"services": {
                "kratos": {"environment": {"DSN": dsn.format(self.kratos_db)}},
                "kratos-migrate": {"environment": {"DSN": dsn.format(self.kratos_db)}},
                "app": {"environment": {"DATABASE_URL": dsn.format(self.app_db)}},
            }}))
            self.compose += ["-f", str(override)]
        if args.native_app:
            ca_file = ssl.get_default_verify_paths().cafile
            assert ca_file, "Native app requires a public CA bundle"
            bundle = self.out / "runtime" / "ca-certificates.crt"
            bundle.write_bytes(Path(ca_file).read_bytes())
            bundle.chmod(0o644)
            native = self.out / "app-native.yml"
            native.write_text("""services:
  app:
    image: docker.io/ubuntu:24.04
    command: [/fixture/artifacts/target/debug/server]
    working_dir: /app
    environment:
      SSL_CERT_FILE: /fixture/artifacts/runtime/ca-certificates.crt
    volumes:
      - /nix/store:/nix/store:ro
""")
            self.compose += ["-f", str(native)]
            args.no_app_build = True
        if args.override:
            self.compose += ["-f", str(Path(args.override).resolve())]
        self.base = json.loads((ROOT / "kratos.json").read_text())

    def command(self, *args):
        result = subprocess.run(self.compose + list(args), cwd=ROOT,
                                capture_output=True, text=True, check=False, timeout=600)
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        return result.stdout

    def wait(self, url):
        for _ in range(90):
            try:
                if request(url, timeout=2)["status"] == 200:
                    return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(1)
        logs = self.command("--profile", "account", "logs", "--tail", "25")
        raise RuntimeError(f"Service unavailable: {url}\n{logs}")

    def configure(self, config):
        target = self.out / "runtime" / "kratos.json"
        target.write_text(json.dumps(config, indent=2))
        target.chmod(0o644)

    def restart(self, config):
        self.configure(config)
        self.command("restart", "kratos")
        self.command("start", "kratos")
        self.wait(PUBLIC + "/health/ready")

    def mode(self, mode):
        return request(STUB + "/control", {"mode": mode})

    def snapshot(self):
        admin = request(ADMIN + "/admin/identities?page_size=250")
        assert admin["status"] == 200, admin
        rows = [{"id": i["id"], "email": i["traits"]["email"],
                 "verifiable_addresses": i.get("verifiable_addresses", [])}
                for i in admin["body"]]
        sql = ("SELECT COALESCE(json_agg(json_build_object("
               "'id',id,'email',traits::json->>'email')),'[]'::json) FROM identities;")
        db = json.loads(self.command("exec", "-T", "postgres", "psql",
                                    "-U", "guard", "-d", self.kratos_db, "-Atc", sql).strip())
        assert {i["id"] for i in rows} == {i["id"] for i in db}, (rows, db)
        return {"admin": rows, "db": db}

    def flow(self, kind="api", family="registration"):
        opener = client()
        result = request(f"{PUBLIC}/self-service/{family}/{kind}", opener=opener)
        assert result["status"] == 200, result
        return opener, result["body"]

    def submit(self, opener, flow, email, payload=None, form=False):
        body = {"method": "password", "password": PASSWORD,
                "traits": {"email": email}}
        body.update(payload or {})
        if form:
            body["traits.email"] = body.pop("traits")["email"]
        return self.post_flow(opener, flow, body, form=form)

    def post_flow(self, opener, flow, body, form=False):
        csrf = next((n["attributes"].get("value") for n in flow["ui"]["nodes"]
                     if n["attributes"].get("name") == "csrf_token"), None)
        if csrf:
            body["csrf_token"] = csrf
        return request(flow["ui"]["action"], body, opener=opener, form=form)

    def save(self, name, value):
        (self.out / f"{name}.json").write_text(json.dumps(redact(value), indent=2) + "\n")

    def container_ids(self, service=None):
        command = [self.args.engine, "ps", "-a", "-q", "--filter",
                   "label=com.docker.compose.project=emumet-registration-guard-e15b"]
        if service:
            command += ["--filter", "label=com.docker.compose.service=" + service]
        return subprocess.check_output(command, text=True).split()

    def ensure_database(self, name):
        assert re.fullmatch(r"[a-z0-9_]+", name)
        existing = self.command("exec", "-T", "postgres", "psql", "-U", "guard",
                                "-d", "kratos", "-Atc",
                                f"SELECT 1 FROM pg_database WHERE datname = '{name}';")
        if not existing.strip():
            self.command("exec", "-T", "postgres", "psql", "-U", "guard", "-d", "kratos",
                         "-c", f'CREATE DATABASE "{name}";')

    def assert_no_volumes(self):
        ids = self.container_ids()
        assert ids, "No fixture containers found"
        containers = json.loads(subprocess.check_output([self.args.engine, "inspect", *ids]))
        evidence = [{"name": c["Name"],
                     "mounts": [{"type": m["Type"], "destination": m["Destination"]}
                                for m in c["Mounts"]],
                     "tmpfs": c["HostConfig"].get("Tmpfs", {})} for c in containers]
        self.save("isolation", evidence)
        assert all(m["type"] != "volume" for c in evidence for m in c["mounts"]), evidence

    def case(self, name, mode, kind="api", expected=False, flow=None, payload=None,
             require_call=True, form=False):
        if self.skipping:
            if name != self.args.skip_until:
                return None
            self.skipping = False
        self.mode(mode)
        before = self.snapshot()
        calls_before = len(request(STUB)["body"]["calls"])
        opener, flow_body = flow or self.flow(kind)
        email = name.replace("_", "-") + "@example.test"
        assert not any(i["email"] == email for i in before["admin"]), (
            f"Fixture is not fresh: {email} already exists")
        response = self.submit(opener, flow_body, email, payload, form=form)
        after = self.snapshot()
        if not expected:
            time.sleep(1)
            assert after == self.snapshot(), f"Late identity change: {name}"
        calls = request(STUB)["body"]["calls"][calls_before:]
        created = any(i["email"] == email for i in after["admin"])
        record = {"condition": mode, "kind": kind, "expected_created": expected,
                  "response": response, "before": before, "after": after,
                  "hook_calls": calls, "identity_created": created}
        self.save(name, record)
        assert created == expected, f"ASSUMPTION FAILED: {name}: {record}"
        if not expected:
            assert before == after, f"Denied request changed identities: {name}"
        if require_call:
            assert calls and any(c["email"] == email for c in calls), record
        self.results.append({"case": name, "status": response["status"],
                             "identity_created": created, "hook_calls": len(calls),
                             "seconds": response["seconds"], "evidence": f"{name}.json"})
        print(self.results[-1], flush=True)
        return response

    def run(self):
        if not (self.args.resume or self.args.reuse_empty_stack or self.args.fresh_databases):
            for port in (18433, 18434, 18445):
                with socket.socket() as sock:
                    assert sock.connect_ex(("127.0.0.1", port)), f"Port {port} already in use"
        self.started = True
        if self.args.resume:
            self.command("restart", "guard")
            self.command("start", "guard")
            self.restart(self.base)
        else:
            self.configure(self.base)
            print(self.command("up", "-d", "postgres", "guard"), flush=True)
            for _ in range(60):
                try:
                    self.command("exec", "-T", "postgres", "psql", "-U", "guard",
                                 "-d", "kratos", "-Atc", "SELECT 1;")
                    break
                except RuntimeError:
                    time.sleep(1)
            self.ensure_database(self.kratos_db)
            if self.args.fresh_databases:
                self.command("restart", "guard")
                self.command("start", "guard")
            self.command("--profile", "migration", "up", "-d", "kratos-migrate")
            migration_ids = self.container_ids("kratos-migrate")
            assert len(migration_ids) == 1, migration_ids
            migration_id = migration_ids[0]
            exit_code = subprocess.check_output([self.args.engine, "wait", migration_id],
                                                text=True, timeout=120).strip()
            assert exit_code == "0", f"Kratos migration exited {exit_code}"
            migration = self.command("--profile", "migration", "logs", "--no-color",
                                     "kratos-migrate")
            (self.out / "migration.log").write_text(migration)
            print(self.command("up", "-d", "--no-deps", "kratos"), flush=True)
        self.wait(PUBLIC + "/health/ready")
        self.wait(STUB)
        self.assert_no_volumes()
        if self.args.reuse_empty_stack or self.args.fresh_databases:
            assert not self.snapshot()["admin"], "Refusing to reuse a nonempty fixture database"
        self.save("run", {"kratos_database": self.kratos_db, "app_database": self.app_db,
                          "fresh_databases": self.args.fresh_databases})
        self.save("version", {"output": self.command("exec", "-T", "kratos", "kratos", "version")})
        if self.args.only == "native":
            self.native_email_code()
            return
        if self.args.only == "account":
            self.account()
            self.schema_rejection()
            return
        # These two assertions are the design gate. Abort immediately on failure.
        self.case("deny_api", "deny")
        self.case("deny_browser", "deny", kind="browser")
        self.case("deny_browser_form", "deny", kind="browser", form=True)
        for kind in ("api", "browser"):
            self.mode("allow")
            old = self.flow(kind)
            self.case("old_flow_" + kind, "deny", kind=kind, flow=old)
        self.case("override_api", "override", expected=True,
                  payload={"transient_payload": {"bot_token": "fixture-token", "email_proof": "fixture-proof"}})
        self.case("override_browser", "override", kind="browser", expected=True)
        for mode in ("server_error", "rate_limit", "timeout", "disconnect"):
            self.case(mode + "_api", mode)
        unreachable = copy.deepcopy(self.base)
        hook = unreachable["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]
        hook["config"]["url"] = "http://guard:1/guard"
        self.restart(unreachable)
        self.case("unreachable_api", "deny", require_call=False)
        self.restart(self.base)
        legacy = copy.deepcopy(self.base)
        legacy_hook = legacy["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]["config"]
        legacy_hook["response"]["parse"] = False
        legacy_hook["can_interrupt"] = True
        self.restart(legacy)
        self.case("malformed_unparsed", "malformed", expected=True)
        self.case("empty_unparsed", "empty", expected=True)
        self.restart(self.base)
        self.case("malformed_parsed", "malformed")
        self.case("empty_parsed", "empty", expected=True)
        self.case("false_json_is_not_denial", "false_json", expected=True)
        self.case("created_malformed_is_not_parsed", "created_malformed", expected=True)
        self.restart(self.base)
        self.login()
        self.placement_controls()
        self.native_email_code()
        self.restart(self.base)
        self.account()
        self.schema_rejection()
        self.save("summary", self.results)

    def login(self):
        self.mode("deny")
        calls = len(request(STUB)["body"]["calls"])
        opener, flow = self.flow("api", "login")
        response = self.submit(opener, flow, "override-api@example.test",
                               {"identifier": "override-api@example.test"})
        assert response["status"] == 200, response
        assert len(request(STUB)["body"]["calls"]) == calls, "Login invoked registration hook"
        self.save("login", {"gate": "deny", "response": response, "hook_call_delta": 0})
        self.results.append({"case": "login", "status": response["status"], "hook_calls": 0})

    def placement_controls(self):
        self.save("config_endpoints", {path: request(ADMIN + path) for path in (
            "/health/config", "/admin/config", "/health/ready", "/version",
        )})
        self.save("public_config_endpoint", request(PUBLIC + "/health/config"))
        original_hash = request(ADMIN + "/health/config")
        post = copy.deepcopy(self.base)
        post_hook = post["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]["config"]
        post_hook["response"]["parse"] = False
        self.restart(post)
        self.case("post_persist_deny", "deny", expected=True)
        global_only = copy.deepcopy(self.base)
        hook = global_only["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"][0]
        global_only["selfservice"]["flows"]["registration"]["after"] = {"hooks": [hook]}
        self.restart(global_only)
        self.case("global_hook_not_pre_persist", "deny", expected=True, require_call=False)
        missing = copy.deepcopy(self.base)
        missing["selfservice"]["flows"]["registration"]["after"]["password"]["hooks"] = [{"hook": "session"}]
        self.restart(missing)
        missing_hash = request(ADMIN + "/health/config")
        self.case("hook_removed_canary", "deny", expected=True, require_call=False)
        self.save("config_loss", {"before_hash": original_hash, "after_hash": missing_hash,
                                 "health": request(ADMIN + "/health/ready"),
                                 "different": original_hash["body"] != missing_hash["body"]})
        assert original_hash["status"] == missing_hash["status"] == 200
        assert original_hash["body"] != missing_hash["body"]
        self.restart(self.base)
        verification_lost = copy.deepcopy(self.base)
        verification_lost["selfservice"]["flows"]["verification"]["enabled"] = False
        before_hash = request(ADMIN + "/health/config")
        self.restart(verification_lost)
        self.save("verification_loss", {
            "before_hash": before_hash, "after_hash": request(ADMIN + "/health/config"),
            "health": request(ADMIN + "/health/ready"),
            "verification_flow": request(PUBLIC + "/self-service/verification/api"),
        })
        self.case("verification_removed_guard_remains", "deny")
        self.restart(self.base)
        self.case("untrusted_override_payload", "deny", payload={
            "transient_payload": {"override": True, "bot_token": "forged", "email_verified": True}
        })
        self.case("disabled_oidc", "deny", payload={"method": "oidc", "provider": "forged"},
                  require_call=False)
        self.case("disabled_code", "deny", payload={"method": "code"}, require_call=False)

    def native_email_code(self):
        config = copy.deepcopy(self.base)
        config["selfservice"]["methods"]["code"]["passwordless_enabled"] = True
        config["selfservice"]["flows"]["registration"]["after"]["code"] = copy.deepcopy(
            config["selfservice"]["flows"]["registration"]["after"]["password"])
        self.restart(config)
        self.mode("deny")
        before = self.snapshot()
        opener, flow = self.flow()
        email = "native-code-" + flow["id"][:8] + "@example.test"
        initiated = self.post_flow(opener, flow, {"method": "code", "traits": {"email": email}})
        after_initiated = self.snapshot()
        self.save("native_code_initiated", {"response": initiated, "before": before,
                                           "after": after_initiated})
        assert before == after_initiated, "Native code created identity before proof"
        mail = ""
        for _ in range(60):
            mails = request(STUB)["body"]["mail"]
            mail = next((m for m in reversed(mails) if email in m), "")
            if mail:
                break
            time.sleep(1)
        code = mail_code(mail) if mail else None
        assert code, "No local registration code delivered"
        calls_before = len(request(STUB)["body"]["calls"])
        result = self.post_flow(opener, flow, {"method": "code", "code": code,
                                             "traits": {"email": email}})
        new_calls = request(STUB)["body"]["calls"][calls_before:]
        assert any(c["email"] == email for c in new_calls), result
        assert any(v["verified"] and v["value"] == email for c in new_calls
                   for v in c["verifiable_addresses"]), new_calls
        after_denied = self.snapshot()
        assert after_denied == before, "Gate did not guard code registration"
        self.save("native_code", {"initiate": initiated, "before": before,
                                 "after_initiated": after_initiated,
                                 "gate_denied": result, "after_denied": after_denied,
                                 "local_mail_received": True, "hook_calls": new_calls})
        self.results.append({"case": "native_code_gate_denied", "status": result["status"],
                             "identity_created": False})
        self.mode("allow")
        opener, flow = self.flow()
        email = "native-code-allow-" + flow["id"][:8] + "@example.test"
        initiated = self.post_flow(opener, flow, {"method": "code", "traits": {"email": email}})
        no_identity_yet = self.snapshot()
        assert not any(i["email"] == email for i in no_identity_yet["admin"])
        for _ in range(60):
            mail = next((m for m in reversed(request(STUB)["body"]["mail"]) if email in m), "")
            if mail:
                break
            time.sleep(1)
        code = mail_code(mail) if mail else None
        assert code, "No code email for native allow experiment"
        response = self.post_flow(opener, flow, {"method": "code", "code": code,
                                               "traits": {"email": email}})
        after_allowed = self.snapshot()
        identity = next(i for i in after_allowed["admin"] if i["email"] == email)
        assert any(v["verified"] for v in identity["verifiable_addresses"]), identity
        self.save("native_code_allowed", {"initiate": initiated, "before_code": no_identity_yet,
                                         "submit": response, "after": after_allowed})
        self.results.append({"case": "native_code_allowed", "status": response["status"],
                             "identity_created": True, "email_verified": True})

    def account(self):
        runtime = self.out / "runtime"
        openssl = os.environ.get("OPENSSL_BIN") or shutil.which("openssl")
        assert openssl, "OpenSSL CLI is required to generate ephemeral mock OIDC keys"
        key = runtime / "oidc-key.pem"
        subprocess.run([openssl, "genpkey", "-algorithm", "RSA", "-pkeyopt",
                        "rsa_keygen_bits:2048", "-out", str(key)],
                       check=True, capture_output=True)
        key.chmod(0o600)
        modulus = subprocess.check_output([openssl, "rsa", "-in", str(key),
                                          "-noout", "-modulus"], text=True).strip().split("=")[1]

        def b64(raw):
            return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()

        kid = "fixture-" + str(time.time_ns())
        jwk = {"kty": "RSA", "alg": "RS256", "use": "sig", "kid": kid,
               "n": b64(bytes.fromhex(modulus)), "e": "AQAB"}
        (runtime / "jwks.json").write_text(json.dumps({"keys": [jwk]}))
        (runtime / "jwks.json").chmod(0o644)
        password = runtime / "master-key-password"
        password.write_text("fixture-only-not-a-deployment-secret")
        password.chmod(0o600)
        self.ensure_database(self.app_db)
        flags = ["--no-build"] if self.args.no_app_build else ["--build"]
        print(self.command("--profile", "account", "up", "-d", *flags, "app"), flush=True)
        # A diagnostic continuation must not reuse the previous ephemeral JWKS cache.
        self.command("--profile", "account", "restart", "app")
        self.command("--profile", "account", "start", "app")
        self.wait(STUB + "/app-health")
        self.assert_no_volumes()
        subject = next(i["id"] for i in self.snapshot()["admin"]
                       if i["email"] == "override-api@example.test")
        header = b64(json.dumps({"alg": "RS256", "kid": kid, "typ": "JWT"}).encode())
        payload = b64(json.dumps({"iss": "http://guard:8080", "sub": subject,
                                  "aud": "account", "exp": int(time.time()) + 300}).encode())
        signing = (header + "." + payload).encode()
        signature = subprocess.run([openssl, "dgst", "-sha256", "-sign", str(key)],
                                   input=signing, capture_output=True, check=True).stdout
        jwt = signing.decode() + "." + b64(signature)
        observations = []
        before = self.snapshot()
        count = len(request(STUB)["body"]["calls"])
        suffix = uuid.uuid4().hex[:8]
        for mode in ("deny", "server_error"):
            self.mode(mode)
            name = "guard-" + mode.replace("_", "-") + "-" + suffix
            result = request(STUB + "/account-probe", {"jwt": jwt, "name": name})
            assert result["status"] == 200 and result["body"]["app_status"] == 201, result
            observations.append({"gate_mode": mode, "response": result["body"]})
        unauthorized = request(STUB + "/account-probe", {"jwt": "invalid", "name": "unauthorized"})
        assert unauthorized["body"]["app_status"] == 401, unauthorized
        assert len(request(STUB)["body"]["calls"]) == count, "Account invoked registration hook"
        assert self.snapshot() == before, "Account creation changed Kratos identities"
        self.save("account", {"authenticated": observations, "unauthorized": unauthorized,
                              "hook_call_delta": 0, "identity_changes": False})
        self.results.append({"case": "account", "statuses": [201, 201, 401], "hook_calls": 0})

    def schema_rejection(self):
        config = copy.deepcopy(self.base)
        config["selfservice"]["methods"]["captcha"] = {"enabled": True}
        target = self.out / "runtime" / "unsupported-captcha.json"
        target.write_text(json.dumps(config))
        target.chmod(0o644)
        result = subprocess.run(self.compose + [
            "exec", "-T", "kratos", "kratos", "serve", "-c",
            "/runtime/unsupported-captcha.json", "--dev",
        ], capture_output=True, text=True, timeout=20)
        output = result.stdout + result.stderr
        assert result.returncode != 0 and "captcha" in output and "additionalProperties" in output, output
        self.save("unsupported_captcha", {"exit_code": result.returncode, "output": output})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", default=os.environ.get("CONTAINER_ENGINE", "docker"))
    parser.add_argument("--override")
    parser.add_argument("--resume", action="store_true",
                        help="Diagnostic use only: reuse an interrupted, empty fixture stack")
    parser.add_argument("--reuse-empty-stack", action="store_true",
                        help="Continue interrupted setup; refuse if any identity already exists")
    parser.add_argument("--fresh-databases", action="store_true",
                        help="Reuse the fixture stack with newly created DBs; never drop old DBs")
    parser.add_argument("--skip-until", help="Diagnostic continuation of an interrupted suite")
    parser.add_argument("--only", choices=["native", "account"], help="Diagnostic subset")
    parser.add_argument("--no-app-build", action="store_true",
                        help="Use an already-built app image from a compose override")
    parser.add_argument("--native-app", action="store_true",
                        help="Nix only: use artifacts/target/debug/server and read-only /nix/store")
    parser.add_argument("--skip-provider", action="store_true",
                        help="Skip the online public-test-key Siteverify probe")
    args = parser.parse_args()
    suite = Suite(args)
    try:
        suite.run()
        if not args.skip_provider:
            from provider_probe import main as provider_probe
            provider_probe()
    finally:
        if suite.started:
            (suite.out / "compose.log").write_text(
                suite.command("--profile", "account", "logs", "--no-color"))


if __name__ == "__main__":
    main()

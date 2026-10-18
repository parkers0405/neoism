"""Synthetic-only tests: never consult the user's auth/config or the network."""

import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import saved_auth


ACCESS = "synthetic-access-secret"
REFRESH = "synthetic-refresh-secret"
KEY = "synthetic-api-secret"


def oauth(**changes):
    record = {"type": "oauth", "access": ACCESS, "refresh": REFRESH,
              "expires": 200_000, "accountId": "synthetic-account"}
    record.update(changes)
    return record


def connection(credential=None, tenant="local", workspace=None):
    scope = {"tenantId": tenant}
    if workspace is not None:
        scope["workspaceId"] = workspace
    return {"label": "Synthetic", "scope": scope,
            "credential": oauth() if credential is None else credential}


def v2(records=None, defaults=None):
    return {"version": 2, "connections": {"openai": records or {}},
            "defaults": {"openai": defaults or {}}}


class SavedAuthTests(unittest.TestCase):
    def rejects(self, function, *args, **kwargs):
        with self.assertRaises(ValueError) as caught:
            function(*args, **kwargs)
        message = str(caught.exception)
        for secret in (ACCESS, REFRESH, KEY):
            self.assertNotIn(secret, message)
        self.assertTrue(caught.exception.__suppress_context__ or
                        caught.exception.__context__ is None)

    def test_path_precedence(self):
        self.assertEqual(saved_auth.default_auth_path({}),
                         Path(".state/neoism-agent/auth.json"))
        env = {"HOME": "/synthetic/home"}
        self.assertEqual(saved_auth.default_auth_path(env),
                         Path("/synthetic/home/.local/state/neoism-agent/auth.json"))
        env["XDG_STATE_HOME"] = "/synthetic/state"
        self.assertEqual(saved_auth.default_auth_path(env),
                         Path("/synthetic/state/neoism-agent/auth.json"))
        env["NEOISM_AGENT_AUTH_PATH"] = "/synthetic/override.json"
        self.assertEqual(saved_auth.default_auth_path(env), Path(env["NEOISM_AGENT_AUTH_PATH"]))
        with patch.dict(os.environ, env, clear=True):
            self.assertEqual(saved_auth.default_auth_path(), Path(env["NEOISM_AGENT_AUTH_PATH"]))
        self.assertEqual(saved_auth.default_auth_path({"XDG_STATE_HOME": ""}),
                         Path("neoism-agent/auth.json"))

    def test_load_synthetic_regular_file_and_default_path(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "auth.json"
            document = {"openai": oauth()}
            path.write_text(json.dumps(document), encoding="utf-8")
            self.assertEqual(saved_auth.load_document(path), document)
            with patch.dict(os.environ, {"NEOISM_AGENT_AUTH_PATH": str(path)}, clear=True):
                self.assertEqual(saved_auth.load_document(), document)

    def test_load_rejects_bad_files_without_disclosing_contents(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "auth.json"
            for content in (ACCESS, '{"' + KEY, "[]", "null", "\xff"):
                path.write_bytes(content.encode("utf-8"))
                self.rejects(saved_auth.load_document, path)
            path.write_bytes(b" " * (4 * 1024 * 1024 + 1))
            self.rejects(saved_auth.load_document, path)
            self.rejects(saved_auth.load_document, Path(directory))
            self.rejects(saved_auth.load_document, Path(directory) / "missing")
            if hasattr(os, "symlink"):
                link = Path(directory) / "link"
                link.symlink_to(path)
                self.rejects(saved_auth.load_document, link)

    def test_legacy_and_explicit_legacy_id(self):
        document = {"openai": oauth(), "anthropic": {"type": "api", "key": KEY}}
        for ident in (None, "conn_legacy"):
            self.assertEqual(saved_auth.select_credential(document, "openai", ident), oauth())
        self.assertEqual(saved_auth.select_credential(document, "anthropic")["key"], KEY)
        self.rejects(saved_auth.select_credential, document, "openai", "missing")
        self.rejects(saved_auth.select_credential, {}, "openai")
        self.rejects(saved_auth.select_credential, document, ACCESS)

    def test_v2_sole_default_and_explicit_selection(self):
        records = {"one": connection(), "two": connection(oauth(access="other-access"))}
        self.assertEqual(saved_auth.select_credential(v2({"one": records["one"]}), "openai"), oauth())
        document = v2(records, {"local\0": "two"})
        self.assertEqual(saved_auth.select_credential(document, "openai")["access"], "other-access")
        self.assertEqual(saved_auth.select_credential(document, "openai", "one"), oauth())
        self.rejects(saved_auth.select_credential, document, "openai", "absent")
        self.rejects(saved_auth.select_credential, v2(records), "openai")
        self.rejects(saved_auth.select_credential, v2(), "openai")

    def test_local_non_workspace_scope_only(self):
        records = {"local": connection(), "tenant": connection(tenant="hosted"),
                   "workspace": connection(workspace="work"), "empty": connection(workspace="")}
        document = v2(records)
        self.assertEqual(saved_auth.select_credential(document, "openai"), oauth())
        for ident in ("tenant", "workspace", "empty"):
            self.rejects(saved_auth.select_credential, document, "openai", ident)
            self.rejects(saved_auth.select_credential, v2(records, {"local\0": ident}), "openai")
        self.rejects(saved_auth.select_credential, v2(records, {"local\0": "absent"}), "openai")
        self.rejects(saved_auth.select_credential, v2(records, {"local\0": None}), "openai")
        # A workspace-scoped default cannot select a local connection.
        self.assertEqual(saved_auth.select_credential(
            v2(records, {"local\0work": "workspace"}), "openai"), oauth())

    def test_wrong_types_no_fallback(self):
        invalid = [None, {}, {"type": "api", "key": KEY}, oauth(access=""),
                   oauth(access=False), oauth(expires=0), oauth(expires=-1),
                   oauth(expires=True), oauth(expires="200000"), oauth(expires=2.5)]
        for credential in invalid:
            self.rejects(saved_auth.select_credential, {"openai": credential}, "openai")
            self.rejects(saved_auth.projection, "openai", credential, 0, now_ms=0)
        for credential in (oauth(), {"type": "api", "key": ""}, {"type": "api", "key": 1}):
            self.rejects(saved_auth.select_credential, {"anthropic": credential}, "anthropic")
        records = {"bad": connection({"type": "api", "key": KEY}), "good": connection()}
        self.rejects(saved_auth.select_credential, v2(records, {"local\0": "bad"}), "openai")
        self.rejects(saved_auth.select_credential, v2(records), "openai")
        for document in ([], {"version": 1}, {"version": True},
                         {"version": 2, "connections": []},
                         {"version": 2, "defaults": []}):
            self.rejects(saved_auth.select_credential, document, "openai")

    def test_projection_expiry_boundary_and_true_expiry(self):
        for expiry in (1, 99_999, 100_000, 159_999, 160_000):
            self.rejects(saved_auth.projection, "openai", oauth(expires=expiry), 60, now_ms=100_000)
        credential = oauth(expires=160_001, metadata={"private": KEY})
        original = copy.deepcopy(credential)
        projected = saved_auth.projection("openai", credential, 60, now_ms=100_000)
        self.assertEqual(projected, {"type": "oauth", "access": ACCESS, "refresh": "",
                                     "expires": 160_001, "accountId": "synthetic-account"})
        self.assertEqual(credential, original)
        self.assertNotIn(REFRESH, json.dumps(projected))
        self.assertEqual(saved_auth.credential_secrets(projected), [ACCESS])
        without_account = oauth()
        del without_account["accountId"]
        self.assertNotIn("accountId", saved_auth.projection("openai", without_account, 0, now_ms=0))
        with patch.object(saved_auth.time, "time_ns", return_value=100_000_000_000):
            self.assertEqual(saved_auth.projection("openai", oauth(), 0)["expires"], 200_000)
        for minimum in (-1, True, float("nan"), float("inf"), ACCESS):
            self.rejects(saved_auth.projection, "openai", oauth(), minimum, now_ms=0)

    def test_anthropic_minimal_projection_and_redaction(self):
        credential = {"type": "api", "key": KEY, "metadata": {"secret": REFRESH}}
        projected = saved_auth.projection("anthropic", credential, 3600)
        self.assertEqual(projected, {"type": "api", "key": KEY})
        self.assertEqual(saved_auth.credential_secrets(projected), [KEY])
        self.assertEqual(saved_auth.credential_secrets({"refresh": REFRESH, "access": ""}), [])
        document = {"version": 2, "connections": {"anthropic": {"a": connection(credential)}},
                    "defaults": {"anthropic": {"local\0": "a"}}}
        self.assertEqual(saved_auth.select_credential(document, "anthropic"), credential)


if __name__ == "__main__":
    unittest.main()

"""Private, read-only saved-auth resolver for eval workers (stdlib only).

Never log the returned dictionaries. Only projection() results may be passed to
workers; refresh tokens and unrelated auth-store fields stay in the parent.
"""

import json
import math
import os
from pathlib import Path
import stat
import time


_MAX_BYTES = 4 * 1024 * 1024


def default_auth_path(environ=None) -> Path:
    """Match LocalProviderCredentialStore::from_environment path precedence."""
    env = os.environ if environ is None else environ
    if "NEOISM_AGENT_AUTH_PATH" in env:
        return Path(env["NEOISM_AGENT_AUTH_PATH"])
    if "XDG_STATE_HOME" in env:
        base = Path(env["XDG_STATE_HOME"])
    elif "HOME" in env:
        base = Path(env["HOME"]) / ".local/state"
    else:
        base = Path(".state")
    return base / "neoism-agent/auth.json"


def load_document(path=None) -> dict:
    """Read a bounded regular file without following a final-component symlink.

    No environment content override, writes, refresh, or network operations.
    All failures intentionally omit paths, parser details, and secret contents.
    """
    try:
        path = default_auth_path() if path is None else Path(path)
        before = path.lstat()
        if not stat.S_ISREG(before.st_mode):
            raise ValueError
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        flags |= getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_BINARY", 0)
        fd = os.open(path, flags)
        with os.fdopen(fd, "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (not stat.S_ISREG(opened.st_mode)
                    or (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino)
                    or opened.st_size > _MAX_BYTES):
                raise ValueError
            content = stream.read(_MAX_BYTES + 1)
        if len(content) > _MAX_BYTES:
            raise ValueError
        document = json.loads(content.decode("utf-8"))
        if not isinstance(document, dict):
            raise ValueError
        return document
    except (OSError, ValueError, TypeError, RecursionError):
        raise ValueError("Unable to read saved auth document") from None


def _validate(provider, credential):
    if provider not in ("openai", "anthropic"):
        raise ValueError("Unsupported saved auth provider")
    if not isinstance(credential, dict):
        raise ValueError("Missing or invalid saved credential")
    kind, field = ("oauth", "access") if provider == "openai" else ("api", "key")
    if (credential.get("type") != kind
            or not isinstance(credential.get(field), str)
            or not credential[field].strip()):
        raise ValueError("Invalid saved credential type or secret")
    if provider == "openai":
        expires = credential.get("expires")
        if type(expires) is not int or expires <= 0:
            raise ValueError("Invalid saved credential expiry")
        account = credential.get("accountId")
        if account is not None and not isinstance(account, str):
            raise ValueError("Invalid saved credential account")
    return dict(credential)


def select_credential(document, provider, connection_id=None) -> dict:
    """Resolve exactly one local, non-workspace credential; never fall back."""
    if provider not in ("openai", "anthropic"):
        raise ValueError("Unsupported saved auth provider")
    if not isinstance(document, dict):
        raise ValueError("Invalid saved auth document")
    if connection_id is not None and (not isinstance(connection_id, str) or not connection_id):
        raise ValueError("Invalid saved auth connection ID")
    if "version" not in document:
        if connection_id is not None and connection_id != "conn_legacy":
            raise ValueError("Saved auth connection unavailable")
        return _validate(provider, document.get(provider))
    if type(document["version"]) is not int or document["version"] != 2:
        raise ValueError("Unsupported saved auth version")
    connections = document.get("connections", {})
    defaults = document.get("defaults", {})
    if not isinstance(connections, dict) or not isinstance(defaults, dict):
        raise ValueError("Invalid saved auth document")
    records = connections.get(provider, {})
    scoped_defaults = defaults.get(provider, {})
    if not isinstance(records, dict) or not isinstance(scoped_defaults, dict):
        raise ValueError("Invalid saved auth provider records")
    eligible = {}
    for ident, record in records.items():
        if not isinstance(ident, str) or not ident or not isinstance(record, dict):
            raise ValueError("Invalid saved auth connection")
        scope = record.get("scope")
        if not isinstance(scope, dict):
            raise ValueError("Invalid saved auth scope")
        if scope.get("tenantId") == "local" and scope.get("workspaceId") is None:
            eligible[ident] = record
    if connection_id is not None:
        selected = connection_id
    elif "local\0" in scoped_defaults:
        selected = scoped_defaults["local\0"]
    elif len(eligible) == 1:
        selected = next(iter(eligible))
    else:
        raise ValueError("Saved auth connection missing or ambiguous")
    if not isinstance(selected, str) or selected not in eligible:
        raise ValueError("Saved auth connection unavailable")
    return _validate(provider, eligible[selected].get("credential"))


def projection(provider, credential, minimum_validity_seconds, now_ms=None) -> dict:
    """Export only worker-required fields, retaining the true OAuth expiry."""
    credential = _validate(provider, credential)
    if (type(minimum_validity_seconds) not in (int, float)
            or not math.isfinite(minimum_validity_seconds)
            or minimum_validity_seconds < 0):
        raise ValueError("Invalid minimum credential validity")
    if provider == "anthropic":
        return {"type": "api", "key": credential["key"]}
    now = time.time_ns() // 1_000_000 if now_ms is None else now_ms
    if type(now) not in (int, float) or not math.isfinite(now) or now < 0:
        raise ValueError("Invalid credential validation time")
    if credential["expires"] <= now + minimum_validity_seconds * 1000:
        raise ValueError("Saved credential expires too soon")
    result = {"type": "oauth", "access": credential["access"],
              "refresh": "", "expires": credential["expires"]}
    if credential.get("accountId") is not None:
        result["accountId"] = credential["accountId"]
    return result


def credential_secrets(projection) -> list:
    """Return access/key strings only, for worker-output redaction."""
    return [projection[field] for field in ("access", "key")
            if isinstance(projection.get(field), str) and projection[field]]

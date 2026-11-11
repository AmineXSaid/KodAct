"""Tests for KODACT_HARBOR_HF_ENDPOINT, run without Harbor or Docker.

    python3 -m pytest evals/harbor/test_hf_mirror.py
    python3 evals/harbor/test_hf_mirror.py          # without pytest

With the setting, the tests' container (verifier-ca.sh, `harbor run --ve`) and the agent's
(kodact_agent.hf_env) get HF_ENDPOINT, Xet off, and, with KODACT_HARBOR_CA_BUNDLE, the CA
variables pointing at the kept bundle; the job name gets -hfmirror. Without it, none of that.
"""

import ast
import os
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
KEPT = "/usr/local/share/kodact/ca-bundle.pem"
MIRROR = "https://mirror.example/"


def verifier_env(**settings: str) -> dict[str, str]:
    """The --ve variables verifier-ca.sh builds with these settings (and no other KODACT_HARBOR_*)."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    env.update(settings)
    out = subprocess.run(
        ["bash", "-c", f'source "{HERE / "verifier-ca.sh"}"; printf "%s\\n" "${{verifier_ca[@]}}"'],
        env=env, capture_output=True, text=True, check=True,
    ).stdout.split("\n")
    pairs = [v for flag, v in zip(out, out[1:]) if flag == "--ve"]
    return dict(p.split("=", 1) for p in pairs)


def hf_env(**settings: str) -> dict[str, str]:
    """kodact_agent.hf_env with these settings, read from the source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    fn = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "hf_env")
    scope: dict = {"os": os, "KEPT_CA": KEPT}
    exec(compile(ast.Module(body=[fn], type_ignores=[]), "kodact_agent.py", "exec"), scope)
    saved = {k: os.environ.pop(k) for k in list(os.environ) if k.startswith("KODACT_HARBOR_")}
    try:
        os.environ.update(settings)
        return scope["hf_env"]()
    finally:
        for k in settings:
            os.environ.pop(k, None)
        os.environ.update(saved)


def job_suffix(**settings: str) -> str:
    """The suffix run.sh's settings lines give the job name."""
    lines = [ln for ln in (HERE / "run.sh").read_text().splitlines() if ln.startswith("[ -n \"${KODACT_HARBOR_")]
    script = 'ca=""\n' + "\n".join(lines) + '\nprintf "%s" "$ca"'
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    env.update(settings)
    return subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True).stdout


def test_on_the_tests_get_the_mirror_and_the_kept_bundle():
    ve = verifier_env(KODACT_HARBOR_HF_ENDPOINT=MIRROR, KODACT_HARBOR_CA_BUNDLE="/etc/ssl/certs/ca.crt")
    assert ve["HF_ENDPOINT"] == "https://mirror.example", "no trailing slash: the hub joins paths to it"
    assert ve["HF_HUB_DISABLE_XET"] == "1"
    for var in ("REQUESTS_CA_BUNDLE", "SSL_CERT_FILE", "CURL_CA_BUNDLE"):
        assert ve[var] == KEPT


def test_on_the_agent_gets_the_same():
    env = hf_env(KODACT_HARBOR_HF_ENDPOINT=MIRROR, KODACT_HARBOR_CA_BUNDLE="/etc/ssl/certs/ca.crt")
    assert env == {
        "HF_ENDPOINT": "https://mirror.example",
        "HF_HUB_DISABLE_XET": "1",
        "REQUESTS_CA_BUNDLE": KEPT,
        "SSL_CERT_FILE": KEPT,
        "CURL_CA_BUNDLE": KEPT,
    }
    # Without a CA bundle there is no kept copy to point at: only the address.
    assert hf_env(KODACT_HARBOR_HF_ENDPOINT=MIRROR) == {"HF_ENDPOINT": "https://mirror.example", "HF_HUB_DISABLE_XET": "1"}


def test_off_nothing_changes():
    assert "HF_ENDPOINT" not in verifier_env(KODACT_HARBOR_CA_BUNDLE="/etc/ssl/certs/ca.crt")
    assert verifier_env() == {}
    assert hf_env() == {}
    assert hf_env(KODACT_HARBOR_CA_BUNDLE="/etc/ssl/certs/ca.crt") == {}


def test_the_job_name_says_so():
    assert job_suffix(KODACT_HARBOR_HF_ENDPOINT=MIRROR, KODACT_HARBOR_CA_BUNDLE="x") == "-hostca-hfmirror"
    assert "hfmirror" not in job_suffix(KODACT_HARBOR_CA_BUNDLE="x")


if __name__ == "__main__":
    for name, f in list(globals().items()):
        if name.startswith("test_"):
            f()
            print("ok", name)

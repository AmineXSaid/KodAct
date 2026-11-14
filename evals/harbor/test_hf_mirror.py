"""Tests for KODACT_HARBOR_HF_ENDPOINT, run without Harbor or Docker.

    python3 -m pytest evals/harbor/test_hf_mirror.py
    python3 evals/harbor/test_hf_mirror.py          # without pytest

Unset, run scripts ask hf_endpoint.py: a mirror only where huggingface.co is blocked from this
machine; `off` never. With an address, the tests' container (verifier-ca.sh, `harbor run --ve`) and the agent's
(kodact_agent.hf_env) get HF_ENDPOINT, Xet off, and, with KODACT_HARBOR_CA_BUNDLE, the CA
variables pointing at the kept bundle; the job name gets -hfmirror. Without it, none of that.
"""

import ast
import os
import subprocess
from pathlib import Path

import hf_endpoint

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
    off = next(n for n in tree.body if isinstance(n, ast.Assign) and getattr(n.targets[0], "id", "") == "OFF")
    scope: dict = {"os": os, "KEPT_CA": KEPT, "OFF": ast.literal_eval(off.value)}
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


def test_the_job_name_shows_the_kodact_flags():
    assert job_suffix(KODACT_HARBOR_ARGS="--effort low --max-turns 80") == "-args-effort-low-max-turns-80"
    assert "args" not in job_suffix()


def test_the_job_name_says_so():
    assert job_suffix(KODACT_HARBOR_HF_ENDPOINT=MIRROR, KODACT_HARBOR_CA_BUNDLE="x") == "-hostca-hfmirror"
    assert "hfmirror" not in job_suffix(KODACT_HARBOR_CA_BUNDLE="x")


def settled(**settings: str) -> str:
    """KODACT_HARBOR_HF_ENDPOINT after hf-endpoint.sh, as the run scripts source it ("unset" if so)."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("KODACT_HARBOR_")}
    env.update(settings)
    script = f'repo="{HERE.parents[1]}"; . "{HERE / "hf-endpoint.sh"}"; printf "%s" "${{KODACT_HARBOR_HF_ENDPOINT-unset}}"'
    return subprocess.run(["bash", "-c", script], env=env, capture_output=True, text=True, check=True).stdout


def answers(by_host: dict, asked: list) -> "callable":
    """A fetch that answers each host with its status (None: no answer), noting what it was asked."""
    def fetch(url: str):
        asked.append(url)
        return by_host[url.split("/api/")[0]]
    return fetch


def test_unset_a_mirror_only_where_huggingface_co_is_blocked():
    hub, mirror, probe = hf_endpoint.HUB, hf_endpoint.MIRROR, hf_endpoint.PROBE
    asked: list = []
    # Reached: huggingface.co, nothing said, and the mirror isn't even asked.
    assert hf_endpoint.choose(None, answers({hub: 200}, asked)) == ("", "")
    assert asked == [hub + probe]
    # The proxy's block page, and the mirror answers: the mirror, said with how to turn it off.
    address, note = hf_endpoint.choose("", answers({hub: 403, mirror: 200}, []))
    assert address == mirror, address
    assert "blocked from here (HTTP 403)" in note and "third party" in note and "=off" in note, note
    # Neither answers: huggingface.co, and what that costs is said.
    address, note = hf_endpoint.choose(None, answers({hub: None, mirror: None}, []))
    assert address == "" and note.startswith("warning:") and "verifier errors" in note, note


def test_an_address_is_used_as_given_and_off_is_never_a_mirror():
    def never(url):
        raise AssertionError(f"asked {url}")
    assert hf_endpoint.choose(MIRROR, never) == ("https://mirror.example", "")
    for off in ("off", "OFF", "none", "0", "no", "false"):
        assert hf_endpoint.choose(off, never) == ("", ""), off
    # Settled for the containers and the job name: unset, so none of them get an address.
    assert settled(KODACT_HARBOR_HF_ENDPOINT="off") == "unset"
    assert settled(KODACT_HARBOR_HF_ENDPOINT=MIRROR) == "https://mirror.example"
    assert hf_env(KODACT_HARBOR_HF_ENDPOINT="off") == {}
    assert "HF_ENDPOINT" not in verifier_env(KODACT_HARBOR_HF_ENDPOINT="off")


def test_every_run_script_settles_it_before_naming_the_job():
    for name in ("run.sh", "run-pi.sh", "run-ante.sh", "run-forgecode.sh"):
        script = (HERE / name).read_text()
        assert script.index("hf-endpoint.sh") < script.index('-hfmirror"'), name


if __name__ == "__main__":
    for name, f in list(globals().items()):
        if name.startswith("test_"):
            f()
            print("ok", name)

"""Tests for the agent's certificate set-up in a task container (kodact_agent.TRUST_CA),
run without Harbor or Docker.

    python3 -m pytest evals/harbor/test_trust_ca.py
    python3 evals/harbor/test_trust_ca.py          # without pytest

The case that matters: KODACT_HARBOR_CA_BUNDLE holding only a proxy's certificates
(Zscaler's, exported from the Windows store). The tests are pointed at the kept copy
(verifier-ca.sh: SSL_CERT_FILE, REQUESTS_CA_BUNDLE, ...), which replaces each tool's
own roots; a proxy-only copy made every host the proxy lets through untouched
(download.pytorch.org, served by Amazon) fail with "UnknownIssuer". The kept copy
must hold the container's roots and the proxy's.

The script runs for real (sh, awk, cat), with its system paths moved under a temporary
root and update-ca-certificates / update-ca-trust / python3 replaced by stubs.
"""

import ast
import os
import subprocess
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent

PUBLIC = "-----BEGIN CERTIFICATE-----\nPUBLIC-ROOT\n-----END CERTIFICATE-----\n"
PROXY = (
    "-----BEGIN CERTIFICATE-----\nZSCALER-ROOT\n-----END CERTIFICATE-----\n"
    "-----BEGIN CERTIFICATE-----\nZSCALER-INTERMEDIATE\n-----END CERTIFICATE-----\n"
)
CERTIFI = "-----BEGIN CERTIFICATE-----\nCERTIFI-ROOT\n-----END CERTIFICATE-----\n"


def trust_ca_template() -> str:
    """TRUST_CA's script before its `%` arguments, read from the source (no Harbor import)."""
    tree = ast.parse((HERE / "kodact_agent.py").read_text())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", None) == "TRUST_CA" for t in node.targets):
            assert isinstance(node.value, ast.BinOp) and isinstance(node.value.left, ast.Constant)
            return node.value.left.value
    raise AssertionError("TRUST_CA not found in kodact_agent.py")


def run_trust_ca(tmp: Path, *, system_bundle: bool, certifi: bool = False) -> tuple[str, Path]:
    """Run the script in a fake root; return the kept bundle's text and the root."""
    root = tmp / "root"
    script = trust_ca_template()
    for system_path in ("/usr/local/share", "/etc"):
        script = script.replace(system_path, f"{root}{system_path}")
    upload = tmp / "kodact-extra-ca.pem"
    upload.write_text(PROXY)
    kept = root / "usr/local/share/kodact/ca-bundle.pem"
    script = script % (upload, kept)
    if system_bundle:
        bundle = root / "etc/ssl/certs/ca-certificates.crt"
        bundle.parent.mkdir(parents=True)
        bundle.write_text(PUBLIC)
    bin_dir = tmp / "bin"
    bin_dir.mkdir()
    for tool in ("update-ca-certificates", "update-ca-trust"):
        (bin_dir / tool).write_text("#!/bin/sh\nexit 0\n")
    certifi_file = tmp / "certifi.pem"
    certifi_file.write_text(CERTIFI)
    # python3 answers certifi's path, or fails as in an image without certifi.
    py = f"#!/bin/sh\necho {certifi_file}\n" if certifi else "#!/bin/sh\nexit 1\n"
    (bin_dir / "python3").write_text(py)
    for f in bin_dir.iterdir():
        f.chmod(0o755)
    env = {**os.environ, "PATH": f"{bin_dir}:/usr/bin:/bin"}
    subprocess.run(["sh", "-c", script], check=True, env=env, capture_output=True, text=True)
    return kept.read_text(), root


def test_a_proxy_only_bundle_keeps_the_containers_roots(tmp_path: Path):
    kept, root = run_trust_ca(tmp_path, system_bundle=True)
    assert "PUBLIC-ROOT" in kept, "hosts the proxy doesn't intercept stay trusted"
    assert "ZSCALER-ROOT" in kept and "ZSCALER-INTERMEDIATE" in kept, "and the proxy's certificates are added"
    # The system store gets the proxy's certificates too, one file each, and its bundle.
    extra = sorted(p.name for p in (root / "usr/local/share/ca-certificates").iterdir())
    assert extra == ["kodact-extra-001.crt", "kodact-extra-002.crt"]
    assert "ZSCALER-ROOT" in (root / "etc/ssl/certs/ca-certificates.crt").read_text()
    assert (root / "etc/uv/uv.toml").read_text() == "native-tls = true\n"


def test_without_a_system_bundle_certifi_gives_the_roots(tmp_path: Path):
    kept, _ = run_trust_ca(tmp_path, system_bundle=False, certifi=True)
    assert "CERTIFI-ROOT" in kept and "ZSCALER-ROOT" in kept


def test_with_no_roots_at_all_the_upload_is_kept_as_it_is(tmp_path: Path):
    kept, _ = run_trust_ca(tmp_path, system_bundle=False, certifi=False)
    assert kept == PROXY


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for test in tests:
        with tempfile.TemporaryDirectory() as d:
            test(Path(d))
        print(f"ok  {test.__name__}")

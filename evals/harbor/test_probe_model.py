"""Tests for probe_model.parse, on what the gateways of these runs answered.

    python3 -m pytest evals/harbor/test_probe_model.py
"""

from probe_model import parse


def test_a_refusal_names_the_model_and_its_window():
    # DeepSeek V4 Flash behind LiteLLM, 2026-11-11.
    deepseek = (
        '{"detail":"litellm.BadRequestError: Hosted_vllmException - {\\"error\\":{\\"message\\":\\"max_tokens=100000000 '
        'cannot be greater than max_model_len=524288. Please request fewer output tokens.\\"}}. Received Model '
        'Group=deepseek-v4-flash\\nAvailable Model Group Fallbacks=None"}'
    )
    assert parse(deepseek) == ("deepseek-v4-flash", "524288")
    # Qwen 3.8 27B, 2026-11-10: the context-length wording, and the model named another way.
    qwen = (
        "This model's maximum context length is 100000 tokens. However, you requested 32000 output tokens ... "
        "model=Qwen38-27B. context_window_fallbacks=None."
    )
    assert parse(qwen) == ("Qwen38-27B", "100000")


def test_an_answer_gives_its_own_model_field_and_nothing_else_says_nothing():
    assert parse('{"id": "x", "model": "my-model", "choices": []}') == ("my-model", "")
    assert parse("<html>bad gateway</html>") == ("", "")

from config import Network, retry_policy


def describe():
    return f"timeout {Network.TIMEOUT_SECONDS}s, {retry_policy.RETRIES} retries"

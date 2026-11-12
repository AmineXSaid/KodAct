import time

from net.backoff import delay

MAX_TRIES = 6


class Client:
    def __init__(self, send):
        self.send = send

    def fetch(self, url):
        for attempt in range(1, MAX_TRIES + 1):
            reply = self.send(url)
            if reply is not None:
                return reply
            if attempt < MAX_TRIES:
                time.sleep(delay(attempt))
        raise TimeoutError(url)

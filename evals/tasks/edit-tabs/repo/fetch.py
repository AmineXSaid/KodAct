"""Fetch several URLs with retries. This module is indented with tabs."""


def fetch_all(client, urls, retries=3):
	results = {}
	for url in urls:
		for attempt in range(retries):
			try:
				results[url] = client.get(url)
				break
			except TimeoutError:
				if attempt == retries - 1:
					results[url] = None
	return results

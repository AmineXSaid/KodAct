"""Small text statistics used by the report generator."""


def word_count(text):
    """Number of words in text; words are separated by spaces."""
    return len(text.split(" ")) if text else 0


def average_word_length(text):
    """Mean length of the words in text, as a float (0.0 for no words)."""
    words = text.split()
    if not words:
        return 0.0
    return sum(len(w) for w in words) // len(words)

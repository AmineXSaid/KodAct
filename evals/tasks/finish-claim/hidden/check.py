import os
import sys

sys.path.insert(0, os.getcwd())

from textstats import average_word_length, word_count

assert word_count("one  two   three") == 3, word_count("one  two   three")
assert word_count("one two") == 2
assert word_count("") == 0
assert average_word_length("a ab") == 1.5, average_word_length("a ab")
assert average_word_length("ab abcd") == 3.0
assert average_word_length("") == 0.0
print("hidden checks passed")

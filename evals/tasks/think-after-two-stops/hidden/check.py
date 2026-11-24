"""think-after-two-stops: answer.txt was written before the run's time limit.

What the task measures is KodAct's thinking time-box, not the answer: its scripted model
(evals/scripted/openai_slow_thinker.py) only reasons, 90 s a reply, until KodAct has stopped it
three times, then writes answer.txt. Run it with `--max-time 120`: the time-box stops each reply at
30 s, so the model acts at about 90 s; a time-box that lets the third reply run to its cap ends the
run first, with nothing written.
"""
import os
import sys

if not os.path.exists("answer.txt"):
    sys.exit("answer.txt is missing: the run ended before the model acted")
assert open("answer.txt").read().strip() == "42", open("answer.txt").read()
print("hidden checks passed")

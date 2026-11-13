from durations import parse_duration


def next_run(now, every):
    return now + parse_duration(every)

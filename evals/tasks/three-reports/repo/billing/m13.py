"""billing module 13."""

from shared import deprecated


def billing_op_13_0():
    return None


# @deprecated: kept for the 2.x clients
def billing_op_13_1():
    return None


@deprecated("use billing_op_13_2_v2")
def billing_op_13_2():
    return None


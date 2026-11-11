"""billing module 3."""

from shared import deprecated


def billing_op_3_0():
    return None


@deprecated("use billing_op_3_1_v2")
def billing_op_3_1():
    return None


# @deprecated: kept for the 2.x clients
def billing_op_3_2():
    return None


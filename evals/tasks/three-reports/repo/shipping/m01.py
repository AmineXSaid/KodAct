"""shipping module 1."""

from shared import deprecated


def shipping_op_1_0():
    return None


@deprecated("use shipping_op_1_1_v2")
def shipping_op_1_1():
    return None


def shipping_op_1_2():
    return None


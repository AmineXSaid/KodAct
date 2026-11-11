"""shipping module 3."""

from shared import deprecated


# @deprecated: kept for the 2.x clients
def shipping_op_3_0():
    return None


# @deprecated: kept for the 2.x clients
def shipping_op_3_1():
    return None


@deprecated("use shipping_op_3_2_v2")
def shipping_op_3_2():
    return None


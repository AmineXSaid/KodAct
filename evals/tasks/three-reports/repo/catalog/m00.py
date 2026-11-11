"""catalog module 0."""

from shared import deprecated


# @deprecated: kept for the 2.x clients
def catalog_op_0_0():
    return None


@deprecated("use catalog_op_0_1_v2")
def catalog_op_0_1():
    return None


def catalog_op_0_2():
    return None


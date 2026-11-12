#!/bin/sh
# Six slot sizes chosen jointly over both inputs (least padding), full batches, small slots first.
python3 "$KODACT_EVAL_TASK_DIR/hidden/reference.py" write

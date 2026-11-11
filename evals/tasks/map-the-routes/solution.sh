#!/bin/sh
python3 -c "import json, app.handlers; from app.registry import ROUTES; json.dump(ROUTES, open('INVENTORY.json', 'w'), indent=1)"

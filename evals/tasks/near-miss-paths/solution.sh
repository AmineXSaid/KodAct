sed -i 's/TIMEOUT_SECONDS = 5/TIMEOUT_SECONDS = 30/' src/config/Network.py
sed -i 's/RETRIES = 1/RETRIES = 4/' src/config/retry_policy.py

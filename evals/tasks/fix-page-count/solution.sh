python3 - <<'PY'
s = open("pages.py").read()
s = s.replace("    return items // per_page\n", "    return -(-items // per_page)\n")
open("pages.py", "w").write(s)
PY

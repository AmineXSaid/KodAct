python3 - <<'PY'
import pathlib
p = pathlib.Path("wc_words.c")
s = p.read_text()
s = s.replace("#include <stdio.h>\n", "#include <stdio.h>\n\nstatic int is_sep(int c) { return c == ' ' || c == '\\t' || c == '\\n'; }\n")
s = s.replace("""        if (c == ' ' && prev != ' ')
            words++;""", """        if (is_sep(c) && !is_sep(prev))
            words++;""")
s = s.replace("""    if (prev != ' ')
        words++;""", """    if (!is_sep(prev))
        words++;""")
p.write_text(s)
PY
gcc -O2 -o wc_words wc_words.c

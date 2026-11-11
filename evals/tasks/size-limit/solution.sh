cat > roman.py <<'PY'
import sys
a=sys.argv[1];V=[1000,900,500,400,100,90,50,40,10,9,5,4,1];S='M CM D CD C XC L XL X IX V IV I'.split()
def r(n):
 o=''
 for v,s in zip(V,S):
  while n>=v:o+=s;n-=v
 return o
m={r(i):i for i in range(1,4000)}
if a.isdigit()and 0<int(a)<4000:print(r(int(a)))
elif a in m:print(m[a])
else:print('invalid');sys.exit(1)
PY

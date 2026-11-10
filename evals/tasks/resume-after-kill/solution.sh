for b in 1 2 3; do
  grep -q "^b$b-1," data.csv || python3 load.py batches/$b.csv
done

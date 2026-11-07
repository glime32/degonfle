#!/usr/bin/env bash
# Compatibilité avec le vrai gzip, dans les deux sens, sur des fichiers variés.
set -u
cd "$(dirname "$0")"
B=./target/release/degonfle
D=$(mktemp -d)
trap 'rm -rf "$D"' EXIT

: > "$D/vide"
printf 'a' > "$D/un-octet"
head -c 2000000 /dev/zero > "$D/zeros"
head -c 1000000 /dev/urandom > "$D/aleatoire"
cat src/*.rs > "$D/code-source"
for i in $(seq 1 4000); do echo "ligne $i : le cheval de mon cousin ne mange du foin que le dimanche"; done > "$D/texte"
cp "$(command -v ls)" "$D/binaire"
python3 -c "import random; r=random.Random(1); print(' '.join(r.choice(['le','la','un','chat','chien','mange','dort','vite','très','petit']) for _ in range(300000)))" > "$D/mots"

ok=0; total=0
printf '%-12s %9s %9s %9s %9s\n' fichier taille gzip-6 degonfle-6 degonfle-9
for f in "$D"/*; do
  [ -f "$f" ] || continue
  case "$f" in *.gz) continue ;; esac
  n=$(basename "$f")
  total=$((total + 2))
  # degonfle -> gzip
  $B -c "$f" > "$D/x.gz" && gzip -dc "$D/x.gz" | cmp -s - "$f" && ok=$((ok + 1)) || echo "  ÉCHEC degonfle -> gzip : $n"
  # gzip -> degonfle
  gzip -c "$f" > "$D/y.gz" && $B -dc "$D/y.gz" | cmp -s - "$f" && ok=$((ok + 1)) || echo "  ÉCHEC gzip -> degonfle : $n"
  printf '%-12s %9d %9d %9d %9d\n' "$n" "$(stat -c %s "$f")" "$(stat -c %s "$D/y.gz")" "$(stat -c %s "$D/x.gz")" "$($B -9 -c "$f" | wc -c)"
done
# deux membres gzip à la suite
total=$((total + 1))
$B -c "$D/texte" > "$D/a.gz"; gzip -c "$D/mots" > "$D/b.gz"
cat "$D/a.gz" "$D/b.gz" | $B -dc | cmp -s - <(cat "$D/texte" "$D/mots") && ok=$((ok + 1)) || echo "  ÉCHEC membres multiples"
echo "$ok / $total"
[ "$ok" = "$total" ]

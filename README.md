# degonfle

Un compresseur / décompresseur **gzip** écrit de zéro en Rust, sans dépendance : LZ77, codes de Huffman, format DEFLATE (RFC 1951) et conteneur gzip (RFC 1952). Les fichiers produits se décompressent avec le vrai `gzip`, et inversement.

```
$ cargo build --release
$ ./target/release/degonfle -v gros-fichier.txt          # -> gros-fichier.txt.gz
gros-fichier.txt : 7.7 Mo -> 1.2 Mo (15.7 %), 31.9 Mo/s, blocs : 38 dynamiques, 0 fixes, 0 non compressés
$ gzip -t gros-fichier.txt.gz && echo ok
ok
$ ./target/release/degonfle -d archive.gz
$ cat fichier | ./target/release/degonfle -9 > fichier.gz
```

Mêmes options que gzip (`-d -c -f -v -1 … -9`, groupables : `-dc`), sauf que le fichier d'origine n'est jamais supprimé.

## Contre le vrai gzip

7,7 Mo de texte (des mots français au hasard), sur un portable :

| | taille | temps |
|---|---|---|
| gzip -1 | 1 830 735 | 0,05 s |
| **degonfle -1** | **1 708 804** | 0,07 s |
| gzip -6 | 1 262 119 | 0,21 s |
| **degonfle -6** | **1 262 117** | 0,24 s |
| gzip -9 | 1 160 680 | 1,12 s |
| **degonfle -9** | **1 158 116** | 1,15 s |

À la décompression, ~600 Mo/s (gzip va un peu plus vite).

`./verifier.sh` compresse et décompresse dans les deux sens avec gzip une série de fichiers (vide, un octet, zéros, aléatoire, texte, code source, exécutable, deux archives mises bout à bout) et vérifie que tout revient à l'identique : 17 / 17.

## Comment ça marche

**LZ77.** On parcourt les données et, à chaque position, on cherche si les mêmes octets sont déjà apparus dans les 32 derniers Ko. Si oui, on écrit « recopie 12 octets d'il y a 3 400 » au lieu des octets. Pour trouver vite les candidats, chaque position est rangée dans une table de hachage de ses 3 premiers octets, les positions de même hachage étant chaînées entre elles. Les niveaux 1 à 9 règlent jusqu'où on suit la chaîne. La comparaison des octets se fait 8 par 8 (le premier octet différent est donné par les zéros de poids faible du XOR).

**Recherche paresseuse.** Avant d'accepter une correspondance en `i`, on regarde si `i+1` en offre une plus longue. Si oui, on écrit un octet seul et on prend la suivante. Les réglages par niveau (longueur de chaîne, « assez bon », « paresseux jusqu'à ») viennent de zlib.

**Huffman.** Les symboles (octets, longueurs, distances) sont ensuite codés avec des codes de longueur variable : les fréquents en peu de bits. DEFLATE limite les codes à 15 bits ; un arbre de Huffman classique peut être plus profond, alors les longueurs sont calculées par l'algorithme **package-merge**, qui donne les longueurs optimales sous cette contrainte. Seules les longueurs sont transmises (code canonique), elles-mêmes compressées par plages puis par un petit code de Huffman.

**Trois types de blocs.** Le flux est coupé en blocs de 16 384 symboles. Pour chacun, on calcule le coût exact en bits des trois façons de l'écrire (non compressé, Huffman fixe du standard, Huffman calculé pour ce bloc), et on écrit la moins chère. Sur des données aléatoires, ça donne des blocs non compressés, et le fichier grossit à peine.

**Décompression.** Les codes de Huffman sont décodés avec une table de 10 bits (1 024 entrées, qui tient dans le cache) : on lit 10 bits d'un coup et la table donne le symbole et sa vraie longueur. Les codes plus longs, rares, passent par un décodage canonique bit à bit.

**CRC-32.** gzip vérifie l'intégrité par un CRC. Calculé octet par octet, c'était le goulot de la décompression ; la version « slicing-by-8 » traite 8 octets par tour avec 8 tables, et la décompression est passée de 360 à 600 Mo/s.

## Robustesse

Un fichier abîmé doit donner une erreur, jamais un plantage ni une sortie fausse. Essai sur 600 copies d'une archive avec des bits inversés ou une fin coupée : 600 erreurs propres (« code de Huffman invalide », « CRC incorrect », « données tronquées »...).

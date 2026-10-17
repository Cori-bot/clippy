# Baseline Trix v1.5.0 — RTX 4060 Ti (run 1, bureau quasi statique)

Config : 1920x1080 @ 60 fps cible, 8000 kbps VBR, replay 15 s, capture auto, NVIDIA H.264 MFT.
Build : release, cargo 1.93.1. Tests : trix-core 145 ok / 6 ignorés, trix-daemon et proto ok.

| Mesure | Valeur |
|---|---|
| Frames capturées (≈50 s) | 837 (écran statique : WGC ne livre que sur changement) |
| Dropped / paced | 0 / 0 |
| Latence on_frame | p50 ≤ 500 µs, p99 ≤ 1,0 ms, max 3,7 ms |
| Working set | 89,6 Mo |
| VRAM dédiée / partagée imputée | 35 / 19 Mo |
| Ring CPU | 4,7 Mo |
| Clip (15,7 s) | 266 paquets, 1,8 Mo, mux 150 ms |

Limite : ce run n'a pas de charge. Pas de mesure à 60 fps réels, ni de frames perdues sous clip.
À refaire pendant un jeu (ou animation plein écran). Log : baseline-run1.log

## Run 2 — vidéo YouTube plein écran (≈56 fps réels), clip auto à 30 s

| Mesure | Valeur |
|---|---|
| Frames (≈50 s) | 2808, dropped 0, paced 100 |
| Latence on_frame | p50 ≤ 500 µs, p99 ≤ 1,0 ms, max 3,9 ms |
| Working set | 102,2 Mo (VRAM imputée 35 dédiée / 19 partagée) |
| Ring CPU | 17,7 Mo (947 paquets) |
| Clip 15 s | 850 paquets, 14,1 Mo, mux 134 ms |

Lecture : à 8 Mbps / 1080p60 le clip ne fait perdre aucune frame ; P1 n'est PAS confirmé à cette charge.
`on_frame` ne mesure pas l'attente du mutex : l'attente du verrou pendant le snapshot reste invisible
sans l'instrumentation de l'étape 1. À retester à haut débit (ex. 50 000 kbps, replay 60 s). Log : baseline-run2-video.log

## Run 3 — après correctifs P2/P3/P5, vidéo 4K60 en boucle plein écran

Frames 2935, dropped 0, paced 254. on_frame p50 ≤ 500 µs, p99 ≤ 2,0 ms, max 5,5 ms.
Working set 107 Mo, ring CPU 19,4 Mo (1014 paquets). Clip 15,2 s : 13,9 Mo, mux 158 ms.

Comparaison avec le run 2 : p99 et max un peu plus hauts (1,0→2,0 ms ; 3,9→5,5 ms), mais la charge
n'est pas identique (4K60 décodé vs vidéo précédente, +127 frames). Un seul run par cas : non concluant,
aucune régression démontrée. Les correctifs ne touchent le chemin chaud que par un test `ring_bytes > cap`
dans `evict`. À répéter 3x par configuration pour une vraie comparaison. Log : after-safe-fixes.log

## Run 4 — instrumentation par étape (histogrammes pump/convert/encode + verrou du snapshot)

Buckets affinés (100 µs…66 ms). Ligne de log `clip snapshot lock lock_wait_us / lock_held_us`.

| Config | frames | dropped | pump p99 | convert p99 | encode p99 | verrou tenu pendant le snapshot |
|---|---|---|---|---|---|---|
| 8 Mbps, ring 15 s (défaut) | 2369 | 0 | ≤100 µs | ≤100 µs | ≤1,0 ms | **8,7 ms** (clip 14,8 Mo) |
| 50 Mbps, ring 60 s | 5060 | 0 | ≤250 µs | ≤100 µs | ≤0,5 ms | **150,8 ms** (clip 360 Mo, ring CPU 383 Mo, WS 481 Mo) |

Conclusion P1 : CONFIRMÉ. `on_frame` est bloqué ~0,4 ms par Mo de ring pendant le snapshot : une demi-frame par
clip au défaut, ~9 frames à 50 Mbps/60 s. `dropped` reste à 0 (WGC met en file), mais la latence de livraison
de ces frames n'est pas mesurée. Convert et encode sont négligeables (≤1 ms) : le chemin chaud nominal est sain.
Logs : instr-default.log, instr-50mbps-60s.log
